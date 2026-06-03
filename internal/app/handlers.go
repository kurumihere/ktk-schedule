package app

import (
	"bytes"
	"context"
	"fmt"
	"log/slog"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	telegram "github.com/go-telegram/bot"
	"github.com/go-telegram/bot/models"

	"ktk-schedule/internal/ktk"
	"ktk-schedule/internal/storage"
	"ktk-schedule/internal/tg"
)

const helpText = `Привет! Я ktk-schedule

Команды:
/start (Показать список команд)
/login логин пароль (Авторизоваться в workspace)
/schedule [дата] (Показать расписание на текущую неделю или дату)
/group 269 (Изменить группу)
/subgroup 1 || 2 (Выбрать первую-вторую подгруппу)
/subgroups_on || _off (Показывать обе подгруппы в одном расписании || Показывать только выбранную подгруппу)
/notify_on || _off (Включить || Отключить утренние уведомления)
`

func (a *App) registerHandlers() {
	a.bot.RegisterHandler(telegram.HandlerTypeMessageText, "start", telegram.MatchTypeCommandStartOnly, a.wrapHandler(a.handleStart))
	a.bot.RegisterHandler(telegram.HandlerTypeMessageText, "my_id", telegram.MatchTypeCommandStartOnly, a.wrapHandler(a.handleMyID))
	a.bot.RegisterHandler(telegram.HandlerTypeMessageText, "login", telegram.MatchTypeCommandStartOnly, a.wrapHandler(a.handleLogin))
	a.bot.RegisterHandler(telegram.HandlerTypeMessageText, "announce", telegram.MatchTypeCommandStartOnly, a.wrapHandler(a.handleAnnounce))
	a.bot.RegisterHandler(telegram.HandlerTypeMessageText, "schedule", telegram.MatchTypeCommandStartOnly, a.wrapHandler(a.handleSchedule))
	a.bot.RegisterHandler(telegram.HandlerTypeMessageText, "group", telegram.MatchTypeCommandStartOnly, a.wrapHandler(a.handleGroup))
	a.bot.RegisterHandler(telegram.HandlerTypeMessageText, "subgroup", telegram.MatchTypeCommandStartOnly, a.wrapHandler(a.handleSubgroup))
	a.bot.RegisterHandler(telegram.HandlerTypeMessageText, "subgroups_on", telegram.MatchTypeCommandStartOnly, a.wrapHandler(a.handleSubgroupsOn))
	a.bot.RegisterHandler(telegram.HandlerTypeMessageText, "subgroups_off", telegram.MatchTypeCommandStartOnly, a.wrapHandler(a.handleSubgroupsOff))
	a.bot.RegisterHandler(telegram.HandlerTypeMessageText, "notify_on", telegram.MatchTypeCommandStartOnly, a.wrapHandler(a.handleNotifyOn))
	a.bot.RegisterHandler(telegram.HandlerTypeMessageText, "notify_off", telegram.MatchTypeCommandStartOnly, a.wrapHandler(a.handleNotifyOff))
	a.bot.RegisterHandler(telegram.HandlerTypeMessageText, "stats", telegram.MatchTypeCommandStartOnly, a.wrapHandler(a.handleStats))
	a.bot.RegisterHandler(telegram.HandlerTypeCallbackQueryData, "schedule:", telegram.MatchTypePrefix, a.wrapHandler(a.handleCallback))
}

func (a *App) wrapHandler(h func(context.Context, *telegram.Bot, *models.Update)) func(context.Context, *telegram.Bot, *models.Update) {
	return func(ctx context.Context, bot *telegram.Bot, update *models.Update) {
		a.activeHandlers.Add(1)
		defer a.activeHandlers.Done()
		h(ctx, bot, update)
	}
}

func (a *App) handleStart(ctx context.Context, _ *telegram.Bot, update *models.Update) {
	if update.Message == nil {
		return
	}
	a.send(ctx, update.Message.Chat.ID, helpText)
}

func (a *App) handleMyID(ctx context.Context, _ *telegram.Bot, update *models.Update) {
	if update.Message == nil {
		return
	}

	a.send(ctx, update.Message.Chat.ID, fmt.Sprintf("Telegram ID: %d", telegramSenderID(update.Message)))
}

func (a *App) handleLogin(ctx context.Context, bot *telegram.Bot, update *models.Update) {
	if update.Message == nil {
		return
	}

	chatID := update.Message.Chat.ID
	a.deleteUserMessage(ctx, bot, update.Message)

	if !a.loginLimiter.allow(chatID) {
		a.send(ctx, chatID, "Слишком много попыток входа. Подожди немного.")
		return
	}

	args := strings.Fields(commandArgs(update.Message.Text))
	if len(args) != 2 {
		a.send(ctx, chatID, "Используй:\n/login логин пароль")
		return
	}

	login := args[0]
	password := args[1]
	if login == "" || password == "" {
		a.send(ctx, chatID, "Логин и пароль не могут быть пустыми.")
		return
	}
	subgroup := clientSubgroupOrDefault(nil, a.cfg.DefaultSubgroup)
	user := storage.User{
		TelegramID:       chatID,
		Login:            login,
		Password:         password,
		GroupID:          a.cfg.DefaultGroup,
		Notify:           false,
		Subgroup:         subgroup,
		ShowAllSubgroups: false,
	}

	client, err := a.authClient(ctx, login, password, user.GroupID)
	if err != nil {
		slog.Error("login failed", "chat_id", chatID, "error", err)
		a.send(ctx, chatID, "Не удалось войти. Проверь логин и пароль.")
		return
	}
	user.Subgroup = clientSubgroupOrDefault(client, a.cfg.DefaultSubgroup)
	user.TeacherHash = client.TeacherHash()

	if err := a.storage.SaveUser(user); err != nil {
		a.send(ctx, chatID, "Не удалось сохранить пользователя: "+err.Error())
		return
	}

	teacher := user.TeacherHash != ""

	halls, err := a.loadLectureHalls(ctx, client, user.GroupID)
	if err != nil {
		slog.Warn("lecture halls load", "error", err)
		halls = make(ktk.LectureHallMap)
	}

	callPresets := a.loadCallPresets(ctx, client)
	absenceMarks := a.loadAbsenceMarks(ctx, client)
	pairTypes := a.loadPairTypes(ctx, client)

	a.setSession(chatID, &Session{
		Client:           client,
		Halls:            halls,
		CallPresets:      callPresets,
		AbsenceMarks:     absenceMarks,
		PairTypes:        pairTypes,
		CurrentIndex:     0,
		Subgroup:         user.Subgroup,
		ShowAllSubgroups: user.ShowAllSubgroups,
		TeacherHash:      user.TeacherHash,
	})

	if teacher {
		a.send(ctx, chatID, "Авторизация успешна (преподаватель).\n\nТеперь напиши /schedule")
	} else {
		a.send(ctx, chatID, fmt.Sprintf("Авторизация успешна.\nГруппа: %d\nПодгруппа: %s\n\nТеперь напиши /schedule", user.GroupID, ktk.SubgroupLabel(user.Subgroup)))
	}
}

func (a *App) deleteUserMessage(ctx context.Context, bot *telegram.Bot, message *models.Message) {
	if bot == nil || message == nil {
		return
	}
	if _, err := bot.DeleteMessage(ctx, &telegram.DeleteMessageParams{
		ChatID:    message.Chat.ID,
		MessageID: message.ID,
	}); err != nil {
		slog.Warn("delete user message", "chat_id", message.Chat.ID, "message_id", message.ID, "error", err)
	}
}

func (a *App) handleSchedule(ctx context.Context, _ *telegram.Bot, update *models.Update) {
	if update.Message == nil {
		return
	}

	chatID := update.Message.Chat.ID
	if !a.rateLimiter.allow(chatID) {
		a.send(ctx, chatID, "Подожди немного перед повторным запросом.")
		return
	}

	user, ok := a.requireUser(ctx, update.Message)
	if !ok {
		return
	}

	targetDate, err := ktk.ParseScheduleDate(commandArgs(update.Message.Text), time.Now(), a.location)
	if err != nil {
		a.send(ctx, chatID, "Не понял дату. Используй /schedule, /schedule 01.09 или /schedule 2026-09-01")
		return
	}

	session, err := a.ensureSession(ctx, user)
	if err != nil {
		slog.Warn("schedule session unavailable, trying persistent cache", "chat_id", chatID, "error", err)
		if a.sendCachedSchedule(ctx, chatID, user, targetDate, "Сайт расписания недоступен, показываю сохранённое расписание.") {
			return
		}
		a.send(ctx, chatID, "Не удалось авторизоваться: "+err.Error())
		return
	}

	displayDays, currentIndex, err := a.refreshSessionSchedule(ctx, user, session, targetDate)
	if err != nil {
		a.circuitBreaker.RecordFailure()
		slog.Error("schedule fetch failed", "chat_id", chatID, "error", err)
		a.send(ctx, chatID, "Не удалось получить расписание. Попробуй позже.")
		return
	}
	a.circuitBreaker.RecordSuccess()
	if len(displayDays) == 0 {
		a.send(ctx, chatID, "Расписание пустое. Попробуй позже.")
		return
	}

	isSchoolDay := ktk.IsSchoolDay(displayDays, targetDate, a.location)
	isNonSchoolDay := isSchoolDay && ktk.IsNonSchoolDay(displayDays[currentIndex])

	if !isSchoolDay || isNonSchoolDay {
		if a.switchToNextWeekSchedule(ctx, session, user.GroupID, user.TeacherHash, targetDate) && !isSchoolDay {
			session.CurrentIndex = -1
		}
		a.setSession(chatID, session)

		a.sendMessage(ctx, &telegram.SendMessageParams{
			ChatID:      chatID,
			Text:        "📅 " + targetDate.In(a.location).Format("02.01.2006") + "\n\nПар нет. Сегодня не учебный день.",
			ReplyMarkup: tg.ScheduleKeyboard(session.Schedule, session.CurrentIndex, session.WeekStart, a.location, 0),
		})
		return
	}

	fileCount := a.fileCountForDay(ctx, displayDays[currentIndex], session)
	a.sendMessage(ctx, &telegram.SendMessageParams{
		ChatID:      chatID,
		Text:        a.formatScheduleDay(ctx, displayDays[currentIndex], session),
		ReplyMarkup: tg.ScheduleKeyboard(displayDays, currentIndex, session.WeekStart, a.location, fileCount),
	})
}

func (a *App) sendCachedSchedule(ctx context.Context, chatID int64, user *storage.User, targetDate time.Time, notice string) bool {
	weekStart := ktk.WeekStart(targetDate, a.location)
	weekKey := weekStart.In(a.location).Format(time.DateOnly)
	cacheTeacherHash := scheduleCacheTeacherHash(user.TeacherHash, user.TeacherHash == "" && a.shouldUseGroupSchedule(user))
	days, err := a.loadPersistentScheduleCache(user.GroupID, weekKey, cacheTeacherHash)
	if err != nil {
		slog.Warn("cached schedule fallback", "chat_id", chatID, "week_start", weekKey, "error", err)
		return false
	}
	if len(days) == 0 {
		return false
	}

	displayDays := days
	if user.TeacherHash == "" {
		displayDays = ktk.FilterScheduleDays(days, user.Subgroup, user.ShowAllSubgroups)
	}
	if len(displayDays) == 0 {
		return false
	}

	currentIndex := ktk.FindDateIndex(displayDays, targetDate, a.location)
	session := &Session{
		AllSchedule:      days,
		Schedule:         displayDays,
		CurrentIndex:     currentIndex,
		WeekStart:        weekStart,
		Subgroup:         user.Subgroup,
		ShowAllSubgroups: user.ShowAllSubgroups,
		TeacherHash:      user.TeacherHash,
	}
	a.setSession(chatID, session)

	text := a.formatScheduleDay(ctx, displayDays[currentIndex], session)
	if notice != "" {
		text = notice + "\n\n" + text
	}
	fileCount := a.fileCountForDay(ctx, displayDays[currentIndex], session)
	a.sendMessage(ctx, &telegram.SendMessageParams{
		ChatID:      chatID,
		Text:        text,
		ReplyMarkup: tg.ScheduleKeyboard(displayDays, currentIndex, weekStart, a.location, fileCount),
	})
	return true
}

func (a *App) handleGroup(ctx context.Context, _ *telegram.Bot, update *models.Update) {
	user, ok := a.requireUser(ctx, update.Message)
	if !ok {
		return
	}

	groupID, err := strconv.Atoi(strings.TrimSpace(commandArgs(update.Message.Text)))
	if err != nil || groupID <= 0 || groupID > 100000 {
		a.send(ctx, user.TelegramID, "Используй:\n/group 269")
		return
	}

	if err := a.storage.SetGroup(user.TelegramID, groupID); err != nil {
		a.send(ctx, user.TelegramID, "Не удалось сохранить группу: "+err.Error())
		return
	}

	a.scheduleCache.invalidate(user.GroupID)
	a.scheduleCache.invalidate(groupID)
	a.deleteSession(user.TelegramID)

	a.send(ctx, user.TelegramID, fmt.Sprintf("Группа изменена на %d.\nТеперь напиши /schedule", groupID))
}

func (a *App) handleSubgroup(ctx context.Context, _ *telegram.Bot, update *models.Update) {
	user, ok := a.requireUser(ctx, update.Message)
	if !ok {
		return
	}

	subgroup, ok := ktk.ParsePersonalSubgroup(commandArgs(update.Message.Text))
	if !ok {
		a.send(ctx, user.TelegramID, "Используй:\n/subgroup 1\nили\n/subgroup 2")
		return
	}

	if err := a.storage.SetSubgroup(user.TelegramID, subgroup); err != nil {
		a.send(ctx, user.TelegramID, "Не удалось сохранить подгруппу: "+err.Error())
		return
	}
	a.modifySession(user.TelegramID, func(s *Session) {
		s.Subgroup = subgroup
		s.ShowAllSubgroups = false
		refilterSessionSchedule(s, a.location)
	})

	a.send(ctx, user.TelegramID, "Подгруппа изменена: "+ktk.SubgroupLabel(subgroup)+".\nТеперь напиши /schedule")
}

func (a *App) handleSubgroupsOn(ctx context.Context, _ *telegram.Bot, update *models.Update) {
	a.handleSubgroupsMode(ctx, update, true)
}

func (a *App) handleSubgroupsOff(ctx context.Context, _ *telegram.Bot, update *models.Update) {
	a.handleSubgroupsMode(ctx, update, false)
}

func (a *App) handleSubgroupsMode(ctx context.Context, update *models.Update, enabled bool) {
	user, ok := a.requireUser(ctx, update.Message)
	if !ok {
		return
	}

	if err := a.storage.SetShowAllSubgroups(user.TelegramID, enabled); err != nil {
		a.send(ctx, user.TelegramID, "Не удалось сохранить режим подгрупп: "+err.Error())
		return
	}
	a.modifySession(user.TelegramID, func(s *Session) {
		s.ShowAllSubgroups = enabled
		refilterSessionSchedule(s, a.location)
	})

	if enabled {
		a.send(ctx, user.TelegramID, "Теперь показываю обе подгруппы.\nНапиши /schedule")
	} else {
		a.send(ctx, user.TelegramID, "Теперь показываю только твою подгруппу: "+ktk.SubgroupLabel(user.Subgroup)+".\nНапиши /schedule")
	}
}

func refilterSessionSchedule(session *Session, loc *time.Location) {
	if session == nil || len(session.AllSchedule) == 0 {
		return
	}

	selectedDate := time.Now()
	if session.CurrentIndex >= 0 && session.CurrentIndex < len(session.Schedule) {
		if !session.WeekStart.IsZero() {
			selectedDate = session.WeekStart.AddDate(0, 0, session.CurrentIndex)
		}
	}

	session.Schedule = ktk.FilterScheduleDays(session.AllSchedule, session.Subgroup, session.ShowAllSubgroups)
	if len(session.Schedule) == 0 {
		session.CurrentIndex = 0
		return
	}
	session.CurrentIndex = ktk.FindDateIndex(session.Schedule, selectedDate, loc)
}

func (a *App) handleNotifyOn(ctx context.Context, _ *telegram.Bot, update *models.Update) {
	a.handleNotify(ctx, update, true)
}

func (a *App) handleNotifyOff(ctx context.Context, _ *telegram.Bot, update *models.Update) {
	a.handleNotify(ctx, update, false)
}

func (a *App) handleStats(ctx context.Context, _ *telegram.Bot, update *models.Update) {
	msg := update.Message
	if msg == nil {
		return
	}

	if a.cfg.OwnerTelegramID == 0 || telegramSenderID(msg) != a.cfg.OwnerTelegramID {
		return
	}

	totalUsers, _ := a.storage.CountUsers()
	notifyUsers, _ := a.storage.CountNotifyUsers()

	activeSessions := a.sessionCount()

	uptime := time.Since(a.startedAt).Round(time.Second)

	text := fmt.Sprintf(`📊 Статистика

👥 Всего пользователей: %d
🔔 Уведомлений: %d
💾 Активных сессий: %d
⏳ Аптайм: %s
🌐 Таймзона: %s`, totalUsers, notifyUsers, activeSessions, formatUptime(uptime), a.cfg.Timezone)

	a.send(ctx, msg.Chat.ID, text)
}

func formatUptime(d time.Duration) string {
	days := int(d.Hours()) / 24
	hours := int(d.Hours()) % 24
	mins := int(d.Minutes()) % 60

	if days > 0 {
		return fmt.Sprintf("%dд %dч %dмин", days, hours, mins)
	}
	if hours > 0 {
		return fmt.Sprintf("%dч %dмин", hours, mins)
	}
	return fmt.Sprintf("%dмин", mins)
}

func (a *App) handleNotify(ctx context.Context, update *models.Update, enabled bool) {
	user, ok := a.requireUser(ctx, update.Message)
	if !ok {
		return
	}

	if err := a.storage.SetNotify(user.TelegramID, enabled); err != nil {
		a.send(ctx, user.TelegramID, "Не удалось сохранить настройку: "+err.Error())
		return
	}

	if enabled {
		a.send(ctx, user.TelegramID, "Утреннее расписание включено.")
	} else {
		a.send(ctx, user.TelegramID, "Утреннее расписание выключено.")
	}
}

func (a *App) handleAnnounce(ctx context.Context, bot *telegram.Bot, update *models.Update) {
	message := update.Message
	if message == nil {
		return
	}

	chatID := message.Chat.ID
	if a.cfg.OwnerTelegramID == 0 {
		a.send(ctx, chatID, "Рассылка отключена. Напиши /my_id и укажи OWNER_TELEGRAM_ID в .env.")
		return
	}
	if telegramSenderID(message) != a.cfg.OwnerTelegramID {
		a.send(ctx, chatID, "Нет доступа.")
		return
	}

	text := commandArgs(message.Text)
	if text == "" && message.ReplyToMessage == nil {
		a.send(ctx, chatID, "Используй:\n/announce текст\n\nИли ответь /announce на сообщение, которое нужно разослать.")
		return
	}

	recipients, err := a.storage.ListUserIDs()
	if err != nil {
		a.send(ctx, chatID, "Не удалось получить список пользователей: "+err.Error())
		return
	}
	if len(recipients) == 0 {
		a.send(ctx, chatID, "Некому отправлять: в базе нет авторизованных пользователей.")
		return
	}

	sent, failed := a.broadcastAnnouncement(ctx, bot, recipients, message, text)
	a.send(ctx, chatID, fmt.Sprintf("Рассылка завершена.\nДоставлено: %d\nОшибок: %d", sent, failed))
}

func (a *App) broadcastAnnouncement(ctx context.Context, bot *telegram.Bot, recipients []int64, message *models.Message, text string) (int, int) {
	var sent atomic.Int64
	var failed atomic.Int64

	sem := make(chan struct{}, announceConcurrency)
	var wg sync.WaitGroup

	for _, recipient := range recipients {
		select {
		case <-ctx.Done():
			wg.Wait()
			return int(sent.Load()), int(failed.Load())
		default:
		}

		sem <- struct{}{}
		wg.Add(1)
		go func(id int64) {
			defer wg.Done()
			defer func() { <-sem }()

			var err error
			if text != "" {
				err = sendMessageWithRetry(ctx, bot, &telegram.SendMessageParams{
					ChatID: id,
					Text:   text,
				})
			} else {
				err = copyMessageWithRetry(ctx, bot, &telegram.CopyMessageParams{
					ChatID:     id,
					FromChatID: message.Chat.ID,
					MessageID:  message.ReplyToMessage.ID,
				})
			}

			if err != nil {
				failed.Add(1)
				slog.Error("announcement delivery", "chat_id", id, "error", err)
			} else {
				sent.Add(1)
			}
		}(recipient)
	}

	wg.Wait()
	return int(sent.Load()), int(failed.Load())
}

func (a *App) handleCallback(ctx context.Context, bot *telegram.Bot, update *models.Update) {
	callback := update.CallbackQuery
	if callback == nil {
		return
	}

	_, _ = bot.AnswerCallbackQuery(ctx, &telegram.AnswerCallbackQueryParams{CallbackQueryID: callback.ID})

	message := callback.Message.Message
	if message == nil {
		a.send(ctx, callback.From.ID, "Сообщение устарело. Напиши /schedule ещё раз.")
		return
	}

	chatID := message.Chat.ID
	session := a.getSession(chatID)
	if session == nil || len(session.Schedule) == 0 {
		user, err := a.storage.GetUser(chatID)
		if err != nil {
			a.send(ctx, chatID, "Ошибка базы данных: "+err.Error())
			return
		}
		if user == nil {
			a.send(ctx, chatID, "Сначала авторизуйся:\n/login логин пароль")
			return
		}

		session, err = a.ensureSession(ctx, user)
		if err != nil {
			slog.Error("session recovery failed", "chat_id", chatID, "error", err)
			a.send(ctx, chatID, "Не удалось восстановить сессию. Попробуй /login.")
			return
		}
	}
	if session.WeekStart.IsZero() {
		session.WeekStart = ktk.WeekStart(time.Now(), a.location)
	}

	data := callback.Data
	oldIndex := session.CurrentIndex

	switch {
	case data == "schedule:week:select":
		a.handleCallbackWeekSelect(ctx, bot, chatID, message.ID, session)
		return
	case strings.HasPrefix(data, "schedule:week:page:"):
		a.handleCallbackWeekPage(ctx, bot, chatID, message.ID, data, session)
		return
	case data == "schedule:back":
		a.editScheduleMessage(ctx, bot, chatID, message.ID, session)
		return
	case data == "schedule:week:prev":
		a.loadScheduleForCallback(ctx, bot, chatID, message.ID, session, a.selectedScheduleDate(session).AddDate(0, 0, -7))
		return
	case data == "schedule:week:next":
		a.loadScheduleForCallback(ctx, bot, chatID, message.ID, session, a.selectedScheduleDate(session).AddDate(0, 0, 7))
		return
	case data == "schedule:week:today":
		a.loadScheduleForCallback(ctx, bot, chatID, message.ID, session, time.Now())
		return
	case strings.HasPrefix(data, "schedule:week:open:"):
		a.handleCallbackWeekOpen(ctx, bot, chatID, message.ID, data, session)
		return
	case data == "schedule:prev":
		a.handleCallbackPrev(session)
	case data == "schedule:next":
		a.handleCallbackNext(session)
	case data == "schedule:today":
		if !a.handleCallbackToday(ctx, bot, chatID, message.ID, session) {
			return
		}
	case strings.HasPrefix(data, "schedule:day:"):
		if !a.handleCallbackDay(data, session) {
			return
		}
	case data == "schedule:download":
		a.handleCallbackDownload(ctx, bot, chatID, session)
		return
	default:
		return
	}

	if session.CurrentIndex < 0 || session.CurrentIndex >= len(session.Schedule) {
		session.CurrentIndex = a.todayIndex(session.Schedule)
	}
	if session.CurrentIndex == oldIndex {
		return
	}
	a.setSession(chatID, session)
	a.editScheduleMessage(ctx, bot, chatID, message.ID, session)
}

func (a *App) handleCallbackWeekSelect(ctx context.Context, bot *telegram.Bot, chatID int64, messageID int, session *Session) {
	session.WeekSelectOffset = 0
	a.setSession(chatID, session)
	a.editWeekSelectMessage(ctx, bot, chatID, messageID, session)
}

func (a *App) handleCallbackWeekPage(ctx context.Context, bot *telegram.Bot, chatID int64, messageID int, data string, session *Session) {
	delta, err := strconv.Atoi(strings.TrimPrefix(data, "schedule:week:page:"))
	if err != nil {
		return
	}
	session.WeekSelectOffset += delta * weekSelectPageSize
	a.setSession(chatID, session)
	a.editWeekSelectMessage(ctx, bot, chatID, messageID, session)
}

func (a *App) handleCallbackWeekOpen(ctx context.Context, bot *telegram.Bot, chatID int64, messageID int, data string, session *Session) {
	weekMillis, err := strconv.ParseInt(strings.TrimPrefix(data, "schedule:week:open:"), 10, 64)
	if err != nil {
		return
	}
	a.loadScheduleForCallback(ctx, bot, chatID, messageID, session, ktk.WeekStartFromMillis(weekMillis, a.location))
}

func (a *App) handleCallbackPrev(session *Session) {
	if session.CurrentIndex > 0 {
		session.CurrentIndex--
	}
}

func (a *App) handleCallbackNext(session *Session) {
	if session.CurrentIndex < len(session.Schedule)-1 {
		session.CurrentIndex++
	}
}

func (a *App) handleCallbackToday(ctx context.Context, bot *telegram.Bot, chatID int64, messageID int, session *Session) bool {
	now := time.Now()
	todayWeekStart := ktk.WeekStart(now, a.location)
	if !session.WeekStart.Equal(todayWeekStart) {
		a.loadScheduleForCallback(ctx, bot, chatID, messageID, session, now)
		return false
	}

	todayIdx := a.todayIndex(session.Schedule)
	isSchoolDay := ktk.IsSchoolDay(session.Schedule, now, a.location)

	if !isSchoolDay || ktk.IsNonSchoolDay(session.Schedule[todayIdx]) {
		user, err := a.storage.GetUser(chatID)
		switched := false
		if err == nil && user != nil {
			switched = a.switchToNextWeekSchedule(ctx, session, user.GroupID, user.TeacherHash, now)
		}
		if !switched {
			session.CurrentIndex = todayIdx
		} else if !isSchoolDay {
			session.CurrentIndex = -1
		}
		a.setSession(chatID, session)
		a.editNonSchoolDayMessage(ctx, bot, chatID, messageID, session, now)
		return false
	}

	session.CurrentIndex = todayIdx
	return true
}

func (a *App) handleCallbackDay(data string, session *Session) bool {
	index, err := strconv.Atoi(strings.TrimPrefix(data, "schedule:day:"))
	if err != nil || index < 0 || index >= len(session.Schedule) {
		return false
	}
	session.CurrentIndex = index
	return true
}

func (a *App) handleCallbackDownload(ctx context.Context, bot *telegram.Bot, chatID int64, session *Session) {
	if session.CurrentIndex < 0 || session.CurrentIndex >= len(session.Schedule) {
		return
	}
	if session.Client == nil {
		a.send(ctx, chatID, "Файлы недоступны без соединения с сайтом расписания.")
		return
	}

	type fileInfo struct {
		ID      int
		Caption string
		Icon    string
	}

	var infos []fileInfo
	seen := make(map[int]bool)

	for _, s := range session.Schedule[session.CurrentIndex].Subjects {
		for _, docID := range s.ExtraData.Homework.Files {
			if seen[docID] {
				continue
			}
			seen[docID] = true
			meta, err := a.documentMetadata(ctx, session, docID)
			if err != nil {
				slog.Error("get file metadata", "doc_id", docID, "error", err)
				infos = append(infos, fileInfo{ID: docID, Caption: fmt.Sprintf("file_%d", docID)})
			} else {
				infos = append(infos, fileInfo{ID: meta.ID, Caption: meta.Caption, Icon: meta.Icon})
			}
		}

		if s.ExtraData.Sheet != 0 {
			if fileID, ok := session.getHomeworkFileID(ctx, s.ExtraData.Sheet); ok && fileID != 0 && !seen[fileID] {
				seen[fileID] = true
				meta, err := a.documentMetadata(ctx, session, fileID)
				if err != nil {
					infos = append(infos, fileInfo{ID: fileID, Caption: fmt.Sprintf("file_%d", fileID)})
				} else {
					infos = append(infos, fileInfo{ID: meta.ID, Caption: meta.Caption, Icon: meta.Icon})
				}
			}
		}
	}

	if len(infos) == 0 {
		a.send(ctx, chatID, "Нет файлов для скачивания.")
		return
	}
	a.setSession(chatID, session)

	var list strings.Builder
	list.WriteString("📎 Файлы:")
	for _, fi := range infos {
		list.WriteByte('\n')
		list.WriteString(fileIconEmoji(fi.Icon))
		list.WriteString(" ")
		list.WriteString(fi.Caption)
	}
	a.send(ctx, chatID, list.String())

	sem := make(chan struct{}, 3)
	var wg sync.WaitGroup

	for _, fi := range infos {
		wg.Add(1)
		go func(id int, name string) {
			defer wg.Done()
			sem <- struct{}{}
			defer func() { <-sem }()
			if err := a.downloadAndSendFile(ctx, bot, chatID, session.Client, id, name); err != nil {
				slog.Error("file download", "doc_id", id, "error", err)
			}
		}(fi.ID, fi.Caption)
	}

	wg.Wait()
}

func fileIconEmoji(icon string) string {
	switch {
	case strings.Contains(icon, "pdf"):
		return "📄"
	case strings.Contains(icon, "image"):
		return "🖼"
	case strings.Contains(icon, "word"):
		return "📝"
	case strings.Contains(icon, "excel"):
		return "📊"
	case strings.Contains(icon, "powerpoint"):
		return "📽"
	case strings.Contains(icon, "archive"):
		return "📦"
	default:
		return "📎"
	}
}

func (a *App) downloadAndSendFile(ctx context.Context, bot *telegram.Bot, chatID int64, client *ktk.Client, docID int, fileName string) error {
	link, caption, err := client.GetFileLink(ctx, docID)
	if err != nil {
		a.send(ctx, chatID, fmt.Sprintf("Ошибка: файл %q не удалось получить", fileName))
		return err
	}
	a.cacheEndpoints(client.Endpoints())

	data, err := client.DownloadFile(ctx, link)
	if err != nil {
		a.send(ctx, chatID, fmt.Sprintf("Ошибка: не удалось скачать %q", fileName))
		return err
	}

	finalName := caption
	if finalName == "" {
		finalName = fileName
	}

	_, err = bot.SendDocument(ctx, &telegram.SendDocumentParams{
		ChatID: chatID,
		Document: &models.InputFileUpload{
			Filename: finalName,
			Data:     bytes.NewReader(data),
		},
	})
	if err != nil {
		a.send(ctx, chatID, fmt.Sprintf("Ошибка: не удалось отправить %q", fileName))
		return err
	}

	return nil
}

func (a *App) editScheduleMessage(ctx context.Context, bot *telegram.Bot, chatID int64, messageID int, session *Session) {
	if session.CurrentIndex < 0 || session.CurrentIndex >= len(session.Schedule) {
		return
	}

	day := session.Schedule[session.CurrentIndex]
	fileCount := a.fileCountForDay(ctx, day, session)
	_, err := bot.EditMessageText(ctx, &telegram.EditMessageTextParams{
		ChatID:      chatID,
		MessageID:   messageID,
		Text:        a.formatScheduleDay(ctx, day, session),
		ReplyMarkup: tg.ScheduleKeyboard(session.Schedule, session.CurrentIndex, session.WeekStart, a.location, fileCount),
	})
	if err != nil {
		if isMessageNotModified(err) {
			return
		}
		slog.Error("edit message", "chat_id", chatID, "error", err)
	}
}

func (a *App) editWeekSelectMessage(ctx context.Context, bot *telegram.Bot, chatID int64, messageID int, session *Session) {
	_, err := bot.EditMessageText(ctx, &telegram.EditMessageTextParams{
		ChatID:      chatID,
		MessageID:   messageID,
		Text:        "Выбери неделю:\n\nСейчас открыта " + ktk.WeekLabel(session.WeekStart, a.location),
		ReplyMarkup: tg.WeekSelectKeyboard(session.WeekStart, session.WeekSelectOffset, a.location),
	})
	if err != nil {
		if isMessageNotModified(err) {
			return
		}
		slog.Error("edit week select message", "chat_id", chatID, "error", err)
	}
}

func (a *App) editEmptyWeekMessage(ctx context.Context, bot *telegram.Bot, chatID int64, messageID int, session *Session) {
	_, err := bot.EditMessageText(ctx, &telegram.EditMessageTextParams{
		ChatID:      chatID,
		MessageID:   messageID,
		Text:        "На этой неделе нет пар.",
		ReplyMarkup: tg.ScheduleKeyboard(nil, 0, session.WeekStart, a.location, 0),
	})
	if err != nil && !isMessageNotModified(err) {
		slog.Error("edit empty week message", "chat_id", chatID, "error", err)
	}
}

func (a *App) editNonSchoolDayMessage(ctx context.Context, bot *telegram.Bot, chatID int64, messageID int, session *Session, date time.Time) {
	text := "📅 " + date.In(a.location).Format("02.01.2006") + "\n\nПар нет. Сегодня не учебный день."
	_, err := bot.EditMessageText(ctx, &telegram.EditMessageTextParams{
		ChatID:      chatID,
		MessageID:   messageID,
		Text:        text,
		ReplyMarkup: tg.ScheduleKeyboard(session.Schedule, session.CurrentIndex, session.WeekStart, a.location, 0),
	})
	if err != nil && !isMessageNotModified(err) {
		slog.Error("edit message", "chat_id", chatID, "error", err)
	}
}

func (a *App) selectedScheduleDate(session *Session) time.Time {
	weekStart := session.WeekStart
	if weekStart.IsZero() {
		weekStart = ktk.WeekStart(time.Now(), a.location)
	}

	index := session.CurrentIndex
	if index < 0 {
		index = 0
	}
	return weekStart.AddDate(0, 0, index)
}

func (a *App) handleDefault(ctx context.Context, _ *telegram.Bot, update *models.Update) {
	if update.Message == nil {
		return
	}
	if a.cfg.OwnerTelegramID != 0 && telegramSenderID(update.Message) == a.cfg.OwnerTelegramID {
		a.send(ctx, update.Message.Chat.ID, "Чтобы разослать это сообщение, ответь на него командой /announce.")
		return
	}
	if update.Message.Text == "" {
		return
	}

	a.send(ctx, update.Message.Chat.ID, "Неизвестная команда. Напиши /start")
}

func commandArgs(text string) string {
	text = strings.TrimSpace(text)
	if text == "" {
		return ""
	}

	index := strings.IndexAny(text, " \t\r\n")
	if index < 0 {
		return ""
	}
	return strings.TrimSpace(text[index+1:])
}

func telegramSenderID(message *models.Message) int64 {
	if message.From != nil {
		return message.From.ID
	}
	return message.Chat.ID
}

func clientSubgroupOrDefault(client *ktk.Client, fallback string) string {
	if client != nil {
		if subgroup, ok := ktk.ParsePersonalSubgroup(client.Subgroup()); ok {
			return subgroup
		}
	}
	if subgroup, ok := ktk.ParsePersonalSubgroup(fallback); ok {
		return subgroup
	}
	return "left"
}
