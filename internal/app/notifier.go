package app

import (
	"context"
	"ktk-schedule/internal/logger"
	"sync"
	"time"

	telegram "github.com/go-telegram/bot"

	"ktk-schedule/internal/ktk"
	"ktk-schedule/internal/storage"
	"ktk-schedule/internal/tg"
)

func (a *App) runNotifier(ctx context.Context) {
	lastRunDate := ""
	if currentDate, ok := a.runDailyScheduleOnce(ctx); ok {
		lastRunDate = currentDate
	}

	ticker := time.NewTicker(time.Minute)
	defer ticker.Stop()

	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			now := time.Now().In(a.location)
			currentTime := now.Format("15:04")
			currentDate := now.Format(time.DateOnly)

			if currentTime != a.cfg.NotifyTime || lastRunDate == currentDate {
				continue
			}

			lastRunDate = currentDate
			a.sendDailySchedules(ctx)
		}
	}
}

func (a *App) runDailyScheduleOnce(ctx context.Context) (string, bool) {
	now := time.Now().In(a.location)
	currentDate, shouldRun, err := startupNotificationDate(now, a.cfg.NotifyTime, a.location)
	if err != nil {
		logger.Warn("Parse notify time: %v", err)
		return "", false
	}
	if !shouldRun {
		return "", false
	}

	logger.Info("Running startup notifications at %s (scheduled for %s)", now.Format("15:04"), a.cfg.NotifyTime)
	a.sendDailySchedules(ctx)
	return currentDate, true
}

func startupNotificationDate(now time.Time, notifyTime string, loc *time.Location) (string, bool, error) {
	now = now.In(loc)
	targetTime, err := time.ParseInLocation("15:04", notifyTime, loc)
	if err != nil {
		return "", false, err
	}

	targetToday := time.Date(now.Year(), now.Month(), now.Day(), targetTime.Hour(), targetTime.Minute(), 0, 0, loc)
	if now.Before(targetToday) || targetToday.Add(2*time.Minute).Before(now) {
		return "", false, nil
	}

	return now.Format(time.DateOnly), true, nil
}

func (a *App) sendDailySchedules(ctx context.Context) {
	if !a.circuitBreaker.Allow() {
		logger.Warn("Circuit breaker open, skipping daily notifications")
		return
	}

	sem := make(chan struct{}, notifyConcurrency)
	var wg sync.WaitGroup
	stopped := false

	err := a.storage.ForEachNotifyUser(func(u *storage.User) error {
		if u.TelegramID <= 0 {
			return nil
		}
		if !a.circuitBreaker.Allow() {
			logger.Warn("Circuit breaker opened during daily notifications")
			stopped = true
			return nil
		}

		sem <- struct{}{}
		wg.Add(1)
		go func(user storage.User) {
			defer wg.Done()
			defer func() { <-sem }()
			a.sendDailyScheduleToUser(ctx, &user)
		}(*u)

		return nil
	})
	if err != nil {
		logger.Error("iterate notify users: %v", err)
	}

	wg.Wait()

	if stopped {
		logger.Warn("Daily notifications stopped early due to circuit breaker")
	}
}

func (a *App) sendDailyScheduleToUser(ctx context.Context, user *storage.User) {
	session, err := a.ensureSession(ctx, user)
	if err != nil {
		a.circuitBreaker.RecordFailure()
		logger.Error("daily schedule session error for chat %v: %v", user.TelegramID, err)
		a.send(ctx, user.TelegramID, "Не удалось обновить утреннее расписание. Попробуй позже.")
		return
	}

	displayDays, index, err := a.refreshSessionSchedule(ctx, user, session, time.Now())
	if err != nil {
		a.circuitBreaker.RecordFailure()
		logger.Error("daily schedule fetch error for chat %v: %v", user.TelegramID, err)
		a.send(ctx, user.TelegramID, "Не удалось получить утреннее расписание. Попробуй позже.")
		return
	}
	a.circuitBreaker.RecordSuccess()
	if len(displayDays) == 0 {
		a.send(ctx, user.TelegramID, "Доброе утро. Расписание на сегодня не найдено.")
		return
	}
	if !ktk.IsSchoolDay(displayDays, time.Now(), a.location) {
		return
	}

	if ktk.IsNonSchoolDay(displayDays[index]) {
		return
	}

	text := "Доброе утро. Расписание на сегодня:\n\n" + a.formatScheduleDay(ctx, displayDays[index], session)
	fileCount := a.fileCountForDay(ctx, displayDays[index], session)
	if err := sendMessageWithRetry(ctx, a.bot, &telegram.SendMessageParams{
		ChatID:      user.TelegramID,
		Text:        text,
		ReplyMarkup: tg.ScheduleKeyboard(displayDays, index, session.WeekStart, a.location, fileCount, 0, session.TeacherHash != "", session.Subgroup, session.ShowAllSubgroups),
	}); err != nil {
		logger.Error("daily schedule delivery for chat %v: %v", user.TelegramID, err)
	}
}
