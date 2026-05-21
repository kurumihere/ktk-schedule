package app

import (
	"context"
	"log/slog"
	"sync"
	"time"

	telegram "github.com/go-telegram/bot"

	"ktk-schedule/internal/ktk"
	"ktk-schedule/internal/storage"
	"ktk-schedule/internal/tg"
)

func (a *App) runNotifier(ctx context.Context) {
	a.runDailyScheduleOnce(ctx)

	ticker := time.NewTicker(time.Minute)
	defer ticker.Stop()

	lastRunDate := ""
	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			now := time.Now().In(a.location)
			currentTime := now.Format("15:04")
			currentDate := now.Format("2006-01-02")

			if currentTime != a.cfg.NotifyTime || lastRunDate == currentDate {
				continue
			}

			lastRunDate = currentDate
			a.sendDailySchedules(ctx)
		}
	}
}

func (a *App) runDailyScheduleOnce(ctx context.Context) {
	now := time.Now().In(a.location)
	targetTime, err := time.ParseInLocation("15:04", a.cfg.NotifyTime, a.location)
	if err != nil {
		slog.Warn("parse notify time", "error", err)
		return
	}

	targetToday := time.Date(now.Year(), now.Month(), now.Day(), targetTime.Hour(), targetTime.Minute(), 0, 0, a.location)
	if now.Before(targetToday) || targetToday.Add(2*time.Minute).Before(now) {
		return
	}

	slog.Info("running startup notification", "current_time", now.Format("15:04"), "notify_time", a.cfg.NotifyTime)
	a.sendDailySchedules(ctx)
}

func (a *App) sendDailySchedules(ctx context.Context) {
	users, err := a.storage.ListNotifyUsers()
	if err != nil {
		slog.Error("list notify users", "error", err)
		return
	}

	if !a.circuitBreaker.Allow() {
		slog.Warn("circuit breaker open, skipping daily notifications")
		return
	}

	sem := make(chan struct{}, notifyConcurrency)
	var wg sync.WaitGroup

	for _, user := range users {
		if !a.circuitBreaker.Allow() {
			slog.Warn("circuit breaker opened during daily notifications")
			break
		}

		sem <- struct{}{}
		wg.Add(1)
		go func(u *storage.User) {
			defer wg.Done()
			defer func() { <-sem }()
			a.sendDailyScheduleToUser(ctx, u)
		}(&user)
	}

	wg.Wait()
}

func (a *App) sendDailyScheduleToUser(ctx context.Context, user *storage.User) {
	session, err := a.ensureSession(ctx, user)
	if err != nil {
		a.circuitBreaker.RecordFailure()
		slog.Error("daily schedule session error", "chat_id", user.TelegramID, "error", err)
		a.send(ctx, user.TelegramID, "Не удалось обновить утреннее расписание. Попробуй позже.")
		return
	}

	displayDays, index, err := a.refreshSessionSchedule(ctx, user, session, time.Now())
	if err != nil {
		a.circuitBreaker.RecordFailure()
		slog.Error("daily schedule fetch error", "chat_id", user.TelegramID, "error", err)
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

	if ktk.AllSubjectsRemote(displayDays[index]) {
		return
	}

	text := "Доброе утро. Расписание на сегодня:\n\n" + a.formatScheduleDay(displayDays[index], session)
	if err := sendMessageWithRetry(ctx, a.bot, &telegram.SendMessageParams{
		ChatID:      user.TelegramID,
		Text:        text,
		ReplyMarkup: tg.ScheduleKeyboard(displayDays, index, session.WeekStart, a.location),
	}); err != nil {
		slog.Error("daily schedule delivery", "chat_id", user.TelegramID, "error", err)
	}
}
