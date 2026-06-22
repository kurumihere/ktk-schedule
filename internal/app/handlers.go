package app

import (
	"bytes"
	"context"
	"fmt"
	"log/slog"
	"regexp"
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
/notify_on || _off (Включить || Отключить утренние уведомления)
`

func (a *App) registerHandlers() {
	a.bot.RegisterHandler(telegram.HandlerTypeMessageText, "start", telegram.MatchTypeCommandStartOnly, a.wrapHandler(a.handleStart))
	a.bot.RegisterHandler(telegram.HandlerTypeMessageText, "my_id", telegram.MatchTypeCommandStartOnly, a.wrapHandler(a.handleMyID))
	a.bot.RegisterHandler(telegram.HandlerTypeMessageText, "login", telegram.MatchTypeCommandStartOnly, a.wrapHandler(a.handleLogin))
	a.bot.RegisterHandler(telegram.HandlerTypeMessageText, "announce", telegram.MatchTypeCommandStartOnly, a.wrapHandler(a.handleAnnounce))
	a.bot.RegisterHandler(telegram.HandlerTypeMessageText, "schedule", telegram.MatchTypeCommandStartOnly, a.wrapHandler(a.handleSchedule))
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
		a.send(ctx, chatID, "Слишком много попыток входа. Попробуй позже.")
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
		PersonalSubgroup: subgroup,
		ShowAllSubgroups: false,
	}

	client, err := a.authClient(ctx, login, password, user.GroupID)
	if err != nil {
		slog.Error("login failed", "chat_id", chatID, "error", err)
		a.send(ctx, chatID, "Не удалось войти. Проверь логин и пароль.")
		return
	}
	if detectedGroupID := client.GroupID(); detectedGroupID > 0 {
		user.GroupID = detectedGroupID
	}
	user.Subgroup = clientSubgroupOrDefault(client, a.cfg.DefaultSubgroup)
	user.PersonalSubgroup = user.Subgroup
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
			ReplyMarkup: tg.ScheduleKeyboard(session.Schedule, session.CurrentIndex, session.WeekStart, a.location, 0, session.ViewingGroupID, session.TeacherHash != "", session.Subgroup, session.ShowAllSubgroups),
		})
		return
	}

	fileCount := a.fileCountForDay(ctx, displayDays[currentIndex], session)
	a.sendMessage(ctx, &telegram.SendMessageParams{
		ChatID:      chatID,
		Text:        a.formatScheduleDay(ctx, displayDays[currentIndex], session),
		ReplyMarkup: tg.ScheduleKeyboard(displayDays, currentIndex, session.WeekStart, a.location, fileCount, session.ViewingGroupID, session.TeacherHash != "", session.Subgroup, session.ShowAllSubgroups),
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
		ReplyMarkup: tg.ScheduleKeyboard(displayDays, currentIndex, weekStart, a.location, fileCount, session.ViewingGroupID, session.TeacherHash != "", session.Subgroup, session.ShowAllSubgroups),
	})
	return true
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
	totalMin := int(d / time.Minute)
	days := totalMin / (24 * 60)
	hours := (totalMin % (24 * 60)) / 60
	mins := totalMin % 60

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

		targetDate := a.extractDateFromMessage(message.Text)
		if targetDate.IsZero() {
			if !session.WeekStart.IsZero() {
				targetDate = session.WeekStart
			} else {
				targetDate = time.Now()
			}
		}

		if _, _, err := a.refreshSessionSchedule(ctx, user, session, targetDate); err != nil {
			slog.Error("schedule recovery failed", "chat_id", chatID, "error", err)
			a.send(ctx, chatID, "Не удалось загрузить расписание после долгого бездействия. Введи /schedule.")
			return
		}
	}
	if session.WeekStart.IsZero() {
		session.WeekStart = ktk.WeekStart(time.Now(), a.location)
	}

	data := callback.Data
	oldIndex := session.CurrentIndex

	switch {
	case data == "schedule:my":
		a.handleCallbackMy(ctx, bot, chatID, message.ID, session)
		return
	case data == "schedule:group:select":
		a.handleCallbackGroupSelect(ctx, bot, chatID, message.ID, session)
		return
	case data == "schedule:subgroup:left":
		a.handleCallbackSubgroup(ctx, bot, chatID, message.ID, session, "left", false)
		return
	case data == "schedule:subgroup:right":
		a.handleCallbackSubgroup(ctx, bot, chatID, message.ID, session, "right", false)
		return
	case data == "schedule:subgroup:all":
		a.handleCallbackSubgroup(ctx, bot, chatID, message.ID, session, "", true)
		return
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
	case data == "schedule:refresh":
		a.editScheduleMessage(ctx, bot, chatID, message.ID, session)
		return
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
		ReplyMarkup: tg.ScheduleKeyboard(session.Schedule, session.CurrentIndex, session.WeekStart, a.location, fileCount, session.ViewingGroupID, session.TeacherHash != "", session.Subgroup, session.ShowAllSubgroups),
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
		ReplyMarkup: tg.ScheduleKeyboard(nil, 0, session.WeekStart, a.location, 0, session.ViewingGroupID, session.TeacherHash != "", session.Subgroup, session.ShowAllSubgroups),
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
		ReplyMarkup: tg.ScheduleKeyboard(session.Schedule, session.CurrentIndex, session.WeekStart, a.location, 0, session.ViewingGroupID, session.TeacherHash != "", session.Subgroup, session.ShowAllSubgroups),
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

func (a *App) extractDateFromMessage(text string) time.Time {
	re := regexp.MustCompile(`\b(\d{2})\.(\d{2})\.(\d{4})\b`)
	if match := re.FindStringSubmatch(text); match != nil {
		t, _ := time.ParseInLocation("02.01.2006", match[0], a.location)
		return t
	}
	return time.Time{}
}

func (a *App) handleDefault(ctx context.Context, _ *telegram.Bot, update *models.Update) {
	if update.Message == nil {
		return
	}
	chatID := update.Message.Chat.ID

	if update.Message.Text == "" {
		return
	}

	// Сначала проверяем ожидание ввода группы — для всех, включая owner
	session := a.getSession(chatID)
	if session != nil && session.AwaitingGroupInput {
		groupID, err := strconv.Atoi(strings.TrimSpace(update.Message.Text))
		if err != nil || groupID <= 0 || groupID > 100000 {
			a.send(ctx, chatID, "Не понял номер группы. Напиши просто число, например: 269")
			return
		}

		a.modifySession(chatID, func(s *Session) {
			s.AwaitingGroupInput = false
		})
		session.AwaitingGroupInput = false

		user, ok := a.requireUser(ctx, update.Message)
		if !ok {
			return
		}
		if !a.rateLimiter.allow(chatID) {
			return
		}
		a.loadGroupScheduleForUser(ctx, user, session, groupID, time.Now(), chatID)
		return
	}

	if a.cfg.OwnerTelegramID != 0 && telegramSenderID(update.Message) == a.cfg.OwnerTelegramID {
		a.send(ctx, chatID, "Чтобы разослать это сообщение, ответь на него командой /announce.")
		return
	}

	a.send(ctx, chatID, "Неизвестная команда. Напиши /start")
}

func (a *App) handleCallbackMy(ctx context.Context, bot *telegram.Bot, chatID int64, messageID int, session *Session) {
	user, err := a.storage.GetUser(chatID)
	if err != nil || user == nil {
		a.send(ctx, chatID, "Ошибка базы данных.")
		return
	}

	targetDate := a.selectedScheduleDate(session)
	sess, err := a.ensureSession(ctx, user)
	if err != nil {
		a.send(ctx, chatID, "Не удалось загрузить расписание. Попробуй /schedule.")
		return
	}

	subgroup, showAllSubgroups := ownScheduleSubgroupSettings(user, sess)
	if user.TeacherHash == "" {
		if err := a.saveUserSubgroupMode(user, subgroup, showAllSubgroups, sess); err != nil {
			a.send(ctx, chatID, "Не удалось сохранить подгруппу.")
			return
		}
	}

	// Полностью сбрасываем состояние просмотра и возвращаем привязанную подгруппу.
	sess.ViewingGroupID = 0
	sess.Subgroup = user.Subgroup
	sess.ShowAllSubgroups = user.ShowAllSubgroups
	sess.AllSchedule = nil
	sess.Schedule = nil
	a.setSession(chatID, sess)

	_, _, err = a.refreshSessionSchedule(ctx, user, sess, targetDate)
	if err != nil {
		a.send(ctx, chatID, "Не удалось загрузить расписание. Попробуй /schedule.")
		return
	}

	a.editScheduleMessage(ctx, bot, chatID, messageID, a.getSession(chatID))
}

func ownScheduleSubgroupSettings(user *storage.User, session *Session) (string, bool) {
	return ownScheduleSubgroupSettingsFromValues(user, detectedPersonalSubgroup(session))
}

func ownScheduleSubgroupSettingsFromValues(user *storage.User, detectedSubgroup string) (string, bool) {
	if user != nil && user.TeacherHash != "" {
		return user.Subgroup, user.ShowAllSubgroups
	}
	return ownScheduleSubgroupFromValues(user, detectedSubgroup), false
}

func ownScheduleSubgroupFromValues(user *storage.User, detectedSubgroup string) string {
	if user == nil {
		return "left"
	}
	if user.TeacherHash != "" {
		return user.Subgroup
	}
	if subgroup, ok := ktk.ParsePersonalSubgroup(detectedSubgroup); ok {
		return subgroup
	}
	if subgroup, ok := ktk.ParsePersonalSubgroup(user.PersonalSubgroup); ok {
		return subgroup
	}
	return clientSubgroupOrDefault(nil, user.Subgroup)
}

func detectedPersonalSubgroup(session *Session) string {
	if session == nil || session.Client == nil {
		return ""
	}
	return session.Client.Subgroup()
}

func (a *App) saveUserSubgroupMode(user *storage.User, subgroup string, showAll bool, session *Session) error {
	user.Subgroup = subgroup
	user.ShowAllSubgroups = showAll
	if user.TeacherHash == "" {
		if detected, ok := ktk.ParsePersonalSubgroup(detectedPersonalSubgroup(session)); ok {
			user.PersonalSubgroup = detected
		} else if _, ok := ktk.ParsePersonalSubgroup(user.PersonalSubgroup); !ok {
			user.PersonalSubgroup = subgroup
		}
	}
	return a.storage.SaveUser(*user)
}

func (a *App) handleCallbackGroupSelect(ctx context.Context, bot *telegram.Bot, chatID int64, messageID int, session *Session) {
	a.modifySession(chatID, func(s *Session) {
		s.AwaitingGroupInput = true
	})
	_, err := bot.EditMessageText(ctx, &telegram.EditMessageTextParams{
		ChatID:    chatID,
		MessageID: messageID,
		Text:      "Напиши номер группы (например: 269)",
		ReplyMarkup: &models.InlineKeyboardMarkup{
			InlineKeyboard: [][]models.InlineKeyboardButton{
				{{Text: "↩️ Назад", CallbackData: "schedule:back"}},
			},
		},
	})
	if err != nil && !isMessageNotModified(err) {
		slog.Error("edit group select message", "chat_id", chatID, "error", err)
	}
}

func (a *App) handleCallbackSubgroup(ctx context.Context, bot *telegram.Bot, chatID int64, messageID int, session *Session, subgroup string, showAll bool) {
	user, err := a.storage.GetUser(chatID)
	if err != nil || user == nil {
		a.send(ctx, chatID, "Ошибка базы данных.")
		return
	}

	targetDate := a.selectedScheduleDate(session)

	if session.ViewingGroupID > 0 {
		// просмотр чужой группы — не трогаем БД, меняем только в памяти сессии
		session.Subgroup = subgroup
		session.ShowAllSubgroups = showAll
		session.Schedule = ktk.FilterScheduleDays(session.AllSchedule, subgroup, showAll)
		session.CurrentIndex = ktk.FindDateIndex(session.Schedule, targetDate, a.location)
		a.setSession(chatID, session)
	} else {
		// своё расписание — сохраняем в БД и перезагружаем
		if err := a.saveUserSubgroupMode(user, subgroup, showAll, session); err != nil {
			a.send(ctx, chatID, "Не удалось сохранить подгруппу.")
			return
		}
		// сбрасываем кеш — иначе loadSchedule вернёт старые данные с персонального эндпоинта
		a.scheduleCache.invalidate(user.GroupID)
		sess, err := a.ensureSession(ctx, user)
		if err != nil {
			a.send(ctx, chatID, "Не удалось загрузить расписание.")
			return
		}
		sess.Subgroup = subgroup
		sess.ShowAllSubgroups = showAll
		if _, _, err = a.refreshSessionSchedule(ctx, user, sess, targetDate); err != nil {
			a.send(ctx, chatID, "Не удалось загрузить расписание.")
			return
		}
	}

	updatedSession := a.getSession(chatID)
	if updatedSession == nil {
		return
	}
	a.editScheduleMessage(ctx, bot, chatID, messageID, updatedSession)
}

func (a *App) loadGroupScheduleForUser(ctx context.Context, user *storage.User, session *Session, groupID int, targetDate time.Time, chatID int64) {
	weekStart := ktk.WeekStart(targetDate, a.location)
	days, err := a.loadSchedule(ctx, session.Client, groupID, "", weekStart, true)
	if err != nil {
		a.circuitBreaker.RecordFailure()
		slog.Error("group schedule fetch failed", "chat_id", chatID, "group_id", groupID, "error", err)
		a.send(ctx, chatID, "Не удалось получить расписание группы. Попробуй позже.")
		return
	}
	a.circuitBreaker.RecordSuccess()

	displayDays := ktk.FilterScheduleDays(days, user.Subgroup, user.ShowAllSubgroups)
	if len(displayDays) == 0 {
		a.send(ctx, chatID, fmt.Sprintf("Расписание группы %d пустое.", groupID))
		return
	}

	currentIndex := ktk.FindDateIndex(displayDays, targetDate, a.location)
	session.AllSchedule = days
	session.Schedule = displayDays
	session.CurrentIndex = currentIndex
	session.WeekStart = weekStart
	session.WeekSelectOffset = 0
	session.ViewingGroupID = groupID
	a.setSession(chatID, session)

	fileCount := a.fileCountForDay(ctx, displayDays[currentIndex], session)
	a.sendMessage(ctx, &telegram.SendMessageParams{
		ChatID:      chatID,
		Text:        fmt.Sprintf("📋 Расписание группы %d\n\n", groupID) + a.formatScheduleDay(ctx, displayDays[currentIndex], session),
		ReplyMarkup: tg.ScheduleKeyboard(displayDays, currentIndex, weekStart, a.location, fileCount, session.ViewingGroupID, session.TeacherHash != "", session.Subgroup, session.ShowAllSubgroups),
	})
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
