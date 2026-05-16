package app

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"net/http"
	"strconv"
	"strings"
	"sync"
	"time"

	telegram "github.com/go-telegram/bot"
	"github.com/go-telegram/bot/models"

	"ktk-schedule/internal/config"
	"ktk-schedule/internal/credentials"
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

const (
	weekSelectPageSize     = 5
	announceSendDelay      = 50 * time.Millisecond
	sessionMaxAge          = 30 * time.Minute
	sessionCleanupInterval = 10 * time.Minute
)

type App struct {
	cfg      config.Config
	bot      *telegram.Bot
	storage  *storage.Storage
	location *time.Location

	endpointsMu sync.RWMutex
	endpoints   ktk.Endpoints

	sessions     sync.Map
	healthServer *http.Server
	rateLimiter  *rateLimiter
	stopCh       chan struct{}
	startedAt    time.Time
}

type Session struct {
	Client           *ktk.Client
	Schedule         []ktk.ScheduleDay
	Halls            ktk.LectureHallMap
	CallPresets      ktk.CallPresetMap
	AbsenceMarks     []ktk.AbsenceMark
	CurrentIndex     int
	WeekStart        time.Time
	WeekSelectOffset int
	Subgroup         string
	ShowAllSubgroups bool
	lastAccess       time.Time
}

func (s *Session) clone() *Session {
	if s == nil {
		return nil
	}
	c := &Session{
		Client:           s.Client,
		Schedule:         make([]ktk.ScheduleDay, len(s.Schedule)),
		CurrentIndex:     s.CurrentIndex,
		WeekStart:        s.WeekStart,
		WeekSelectOffset: s.WeekSelectOffset,
		Subgroup:         s.Subgroup,
		ShowAllSubgroups: s.ShowAllSubgroups,
	}
	copy(c.Schedule, s.Schedule)

	for i := range c.Schedule {
		if len(s.Schedule[i].Subjects) > 0 {
			c.Schedule[i].Subjects = make([]ktk.ScheduleItem, len(s.Schedule[i].Subjects))
			copy(c.Schedule[i].Subjects, s.Schedule[i].Subjects)
		}
	}

	if s.Halls != nil {
		c.Halls = make(ktk.LectureHallMap, len(s.Halls))
		for k, v := range s.Halls {
			c.Halls[k] = v
		}
	}

	if s.CallPresets != nil {
		c.CallPresets = make(ktk.CallPresetMap, len(s.CallPresets))
		for k, v := range s.CallPresets {
			c.CallPresets[k] = v
		}
	}

	if s.AbsenceMarks != nil {
		c.AbsenceMarks = make([]ktk.AbsenceMark, len(s.AbsenceMarks))
		copy(c.AbsenceMarks, s.AbsenceMarks)
	}

	return c
}

func New(cfg config.Config) (*App, error) {
	credentialCipher, err := credentials.New(cfg.CredentialsSecret)
	if err != nil {
		return nil, err
	}

	store, err := storage.New(cfg.DatabasePath, credentialCipher)
	if err != nil {
		return nil, err
	}

	location, err := time.LoadLocation(cfg.Timezone)
	if err != nil {
		_ = store.Close()
		return nil, err
	}

	app := &App{
		cfg:      cfg,
		storage:  store,
		location: location,
		endpoints: ktk.Endpoints{
			SignInPath:      cfg.KTKSignInPath,
			SchedulePath:    cfg.KTKSchedulePath,
			LectureHallPath: cfg.KTKLectureHallPath,
			CallPresetPath:  cfg.KTKCallPresetPath,
			BranchID:        cfg.KTKBranchID,
		},
		rateLimiter: newRateLimiter(),
		stopCh:      make(chan struct{}),
		startedAt:   time.Now(),
	}

	mux := http.NewServeMux()
	mux.HandleFunc("/health", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusOK)
		fmt.Fprint(w, `{"status":"ok"}`)
	})
	app.healthServer = &http.Server{
		Addr:        ":" + cfg.HealthPort,
		Handler:     mux,
		ReadTimeout: 5 * time.Second,
	}

	bot, err := telegram.New(
		cfg.BotToken,
		telegram.WithAllowedUpdates(telegram.AllowedUpdates{
			models.AllowedUpdateMessage,
			models.AllowedUpdateCallbackQuery,
		}),
		telegram.WithDefaultHandler(app.handleDefault),
		telegram.WithErrorsHandler(func(err error) {
			slog.Error("telegram error", "error", err)
		}),
	)
	if err != nil {
		_ = store.Close()
		return nil, fmt.Errorf("initialize telegram bot: %w; check BOT_TOKEN from @BotFather", err)
	}

	app.bot = bot
	app.registerHandlers()
	return app, nil
}

func (a *App) registerHandlers() {
	a.bot.RegisterHandler(telegram.HandlerTypeMessageText, "start", telegram.MatchTypeCommandStartOnly, a.handleStart)
	a.bot.RegisterHandler(telegram.HandlerTypeMessageText, "my_id", telegram.MatchTypeCommandStartOnly, a.handleMyID)
	a.bot.RegisterHandler(telegram.HandlerTypeMessageText, "login", telegram.MatchTypeCommandStartOnly, a.handleLogin)
	a.bot.RegisterHandler(telegram.HandlerTypeMessageText, "announce", telegram.MatchTypeCommandStartOnly, a.handleAnnounce)
	a.bot.RegisterHandler(telegram.HandlerTypeMessageText, "schedule", telegram.MatchTypeCommandStartOnly, a.handleSchedule)
	a.bot.RegisterHandler(telegram.HandlerTypeMessageText, "group", telegram.MatchTypeCommandStartOnly, a.handleGroup)
	a.bot.RegisterHandler(telegram.HandlerTypeMessageText, "subgroup", telegram.MatchTypeCommandStartOnly, a.handleSubgroup)
	a.bot.RegisterHandler(telegram.HandlerTypeMessageText, "subgroups_on", telegram.MatchTypeCommandStartOnly, a.handleSubgroupsOn)
	a.bot.RegisterHandler(telegram.HandlerTypeMessageText, "subgroups_off", telegram.MatchTypeCommandStartOnly, a.handleSubgroupsOff)
	a.bot.RegisterHandler(telegram.HandlerTypeMessageText, "notify_on", telegram.MatchTypeCommandStartOnly, a.handleNotifyOn)
	a.bot.RegisterHandler(telegram.HandlerTypeMessageText, "notify_off", telegram.MatchTypeCommandStartOnly, a.handleNotifyOff)
	a.bot.RegisterHandler(telegram.HandlerTypeMessageText, "stats", telegram.MatchTypeCommandStartOnly, a.handleStats)
	a.bot.RegisterHandler(telegram.HandlerTypeCallbackQueryData, "schedule:", telegram.MatchTypePrefix, a.handleCallback)
}

func (a *App) Close() {
	close(a.stopCh)
	a.rateLimiter.Close()
	if err := a.storage.Close(); err != nil {
		slog.Error("storage close", "error", err)
	}
}

func (a *App) sessionCleanupLoop() {
	ticker := time.NewTicker(sessionCleanupInterval)
	defer ticker.Stop()

	for {
		select {
		case <-a.stopCh:
			return
		case <-ticker.C:
			a.cleanupSessions()
		}
	}
}

func (a *App) cleanupSessions() {
	now := time.Now()
	a.sessions.Range(func(key, value any) bool {
		s := value.(*Session)
		if now.Sub(s.lastAccess) > sessionMaxAge {
			a.sessions.Delete(key)
		}
		return true
	})
}

func (a *App) Run(ctx context.Context) error {
	if me, err := a.bot.GetMe(ctx); err == nil {
		slog.Info("bot started", "username", me.Username)
	} else {
		slog.Warn("bot started", "error", err)
	}

	go a.runNotifier(ctx)
	go a.sessionCleanupLoop()
	go a.healthServer.ListenAndServe()
	a.bot.Start(ctx)

	shutdownCtx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	if err := a.healthServer.Shutdown(shutdownCtx); err != nil {
		slog.Error("health server shutdown", "error", err)
	}

	return nil
}

func (a *App) getSession(telegramID int64) *Session {
	value, ok := a.sessions.Load(telegramID)
	if !ok {
		return nil
	}

	s := value.(*Session).clone()
	s.lastAccess = time.Now()
	return s
}

func (a *App) setSession(telegramID int64, session *Session) {
	if session == nil {
		return
	}

	session.lastAccess = time.Now()
	a.sessions.Store(telegramID, session.clone())
}

func (a *App) cachedEndpoints() ktk.Endpoints {
	a.endpointsMu.RLock()
	defer a.endpointsMu.RUnlock()
	return a.endpoints
}

func (a *App) cacheEndpoints(endpoints ktk.Endpoints) {
	if !hasScheduleEndpoint(endpoints) {
		return
	}

	a.endpointsMu.Lock()
	defer a.endpointsMu.Unlock()
	a.endpoints = endpoints
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

func (a *App) handleLogin(ctx context.Context, _ *telegram.Bot, update *models.Update) {
	if update.Message == nil {
		return
	}

	chatID := update.Message.Chat.ID
	args := strings.Fields(commandArgs(update.Message.Text))
	if len(args) != 2 {
		a.send(ctx, chatID, "Используй:\n/login логин пароль")
		return
	}

	login := args[0]
	password := args[1]
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
		a.send(ctx, chatID, "Не удалось войти: "+err.Error())
		return
	}
	user.Subgroup = clientSubgroupOrDefault(client, a.cfg.DefaultSubgroup)

	if err := a.storage.SaveUser(user); err != nil {
		a.send(ctx, chatID, "Не удалось сохранить пользователя: "+err.Error())
		return
	}

	halls, err := a.loadLectureHalls(ctx, client, user.GroupID)
	if err != nil {
		slog.Warn("lecture halls load", "error", err)
		halls = make(ktk.LectureHallMap)
	}

	callPresets := a.loadCallPresets(ctx, client)
	absenceMarks := a.loadAbsenceMarks(ctx, client)

	a.setSession(chatID, &Session{
		Client:           client,
		Halls:            halls,
		CallPresets:      callPresets,
		AbsenceMarks:     absenceMarks,
		CurrentIndex:     0,
		Subgroup:         user.Subgroup,
		ShowAllSubgroups: user.ShowAllSubgroups,
	})

	a.send(ctx, chatID, fmt.Sprintf("Авторизация успешна.\nГруппа: %d\nПодгруппа: %s\n\nТеперь напиши /schedule", user.GroupID, ktk.SubgroupLabel(user.Subgroup)))
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

	session, err := a.ensureSession(ctx, *user)
	if err != nil {
		a.send(ctx, chatID, "Не удалось авторизоваться: "+err.Error())
		return
	}

	targetDate, err := ktk.ParseScheduleDate(commandArgs(update.Message.Text), time.Now(), a.location)
	if err != nil {
		a.send(ctx, chatID, "Не понял дату. Используй /schedule, /schedule 01.09 или /schedule 2026-09-01")
		return
	}

	displayDays, currentIndex, err := a.refreshSessionSchedule(ctx, *user, session, targetDate)
	if err != nil {
		a.send(ctx, chatID, "Не удалось получить расписание: "+err.Error())
		return
	}
	if len(displayDays) == 0 {
		a.send(ctx, chatID, "Расписание пустое. Попробуй позже.")
		return
	}

	if !ktk.IsSchoolDay(displayDays, targetDate, a.location) {
		a.sendMessage(ctx, &telegram.SendMessageParams{
			ChatID:      chatID,
			Text:        "📅 " + targetDate.In(a.location).Format("02.01.2006") + "\n\nПар нет. Сегодня не учебный день.",
			ReplyMarkup: tg.ScheduleKeyboard(displayDays, currentIndex, session.WeekStart, a.location),
		})
		return
	}

	a.sendMessage(ctx, &telegram.SendMessageParams{
		ChatID:      chatID,
		Text:        a.formatScheduleDay(displayDays[currentIndex], session),
		ReplyMarkup: tg.ScheduleKeyboard(displayDays, currentIndex, session.WeekStart, a.location),
	})
}

func (a *App) handleGroup(ctx context.Context, _ *telegram.Bot, update *models.Update) {
	user, ok := a.requireUser(ctx, update.Message)
	if !ok {
		return
	}

	groupID, err := strconv.Atoi(strings.TrimSpace(commandArgs(update.Message.Text)))
	if err != nil || groupID <= 0 {
		a.send(ctx, user.TelegramID, "Используй:\n/group 269")
		return
	}

	if err := a.storage.SetGroup(user.TelegramID, groupID); err != nil {
		a.send(ctx, user.TelegramID, "Не удалось сохранить группу: "+err.Error())
		return
	}

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
	if session := a.getSession(user.TelegramID); session != nil {
		session.Subgroup = subgroup
		session.ShowAllSubgroups = false
		a.setSession(user.TelegramID, session)
	}

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
	if session := a.getSession(user.TelegramID); session != nil {
		session.ShowAllSubgroups = enabled
		a.setSession(user.TelegramID, session)
	}

	if enabled {
		a.send(ctx, user.TelegramID, "Теперь показываю обе подгруппы.\nНапиши /schedule")
	} else {
		a.send(ctx, user.TelegramID, "Теперь показываю только твою подгруппу: "+ktk.SubgroupLabel(user.Subgroup)+".\nНапиши /schedule")
	}
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

	var activeSessions int
	a.sessions.Range(func(_, _ any) bool {
		activeSessions++
		return true
	})

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
	sent := 0
	failed := 0
	ticker := time.NewTicker(announceSendDelay)
	defer ticker.Stop()

	for i, recipient := range recipients {
		if i > 0 {
			select {
			case <-ctx.Done():
				return sent, failed + len(recipients) - i
			case <-ticker.C:
			}
		}

		var err error
		if text != "" {
			_, err = bot.SendMessage(ctx, &telegram.SendMessageParams{
				ChatID: recipient,
				Text:   text,
			})
		} else {
			_, err = bot.CopyMessage(ctx, &telegram.CopyMessageParams{
				ChatID:     recipient,
				FromChatID: message.Chat.ID,
				MessageID:  message.ReplyToMessage.ID,
			})
		}

		if err != nil {
			failed++
			slog.Error("announcement delivery", "chat_id", recipient, "error", err)
			continue
		}
		sent++
	}

	return sent, failed
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
	if session == nil || session.Client == nil || len(session.Schedule) == 0 {
		user, err := a.storage.GetUser(chatID)
		if err != nil {
			a.send(ctx, chatID, "Ошибка базы данных: "+err.Error())
			return
		}
		if user == nil {
			a.send(ctx, chatID, "Сначала авторизуйся:\n/login логин пароль")
			return
		}

		session, err = a.ensureSession(ctx, *user)
		if err != nil {
			a.send(ctx, chatID, "Не удалось восстановить сессию: "+err.Error())
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
	todayWeekStart := ktk.WeekStart(time.Now(), a.location)
	if !session.WeekStart.Equal(todayWeekStart) {
		a.loadScheduleForCallback(ctx, bot, chatID, messageID, session, time.Now())
		return false
	}
	if !ktk.IsSchoolDay(session.Schedule, time.Now(), a.location) {
		session.CurrentIndex = a.todayIndex(session.Schedule)
		a.setSession(chatID, session)
		a.editNonSchoolDayMessage(ctx, bot, chatID, messageID, session, time.Now())
		return false
	}
	session.CurrentIndex = a.todayIndex(session.Schedule)
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

func (a *App) editScheduleMessage(ctx context.Context, bot *telegram.Bot, chatID int64, messageID int, session *Session) {
	if session.CurrentIndex < 0 || session.CurrentIndex >= len(session.Schedule) {
		return
	}

	day := session.Schedule[session.CurrentIndex]
	_, err := bot.EditMessageText(ctx, &telegram.EditMessageTextParams{
		ChatID:      chatID,
		MessageID:   messageID,
		Text:        a.formatScheduleDay(day, session),
		ReplyMarkup: tg.ScheduleKeyboard(session.Schedule, session.CurrentIndex, session.WeekStart, a.location),
	})
	if err != nil {
		if isMessageNotModified(err) {
			return
		}
		slog.Error("edit message", "error", err)
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
		slog.Error("edit week select message", "error", err)
	}
}

func (a *App) editNonSchoolDayMessage(ctx context.Context, bot *telegram.Bot, chatID int64, messageID int, session *Session, date time.Time) {
	text := "📅 " + date.In(a.location).Format("02.01.2006") + "\n\nПар нет. Сегодня не учебный день."
	_, err := bot.EditMessageText(ctx, &telegram.EditMessageTextParams{
		ChatID:      chatID,
		MessageID:   messageID,
		Text:        text,
		ReplyMarkup: tg.ScheduleKeyboard(session.Schedule, session.CurrentIndex, session.WeekStart, a.location),
	})
	if err != nil && !isMessageNotModified(err) {
		slog.Error("edit message", "error", err)
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

func (a *App) ensureSession(ctx context.Context, user storage.User) (*Session, error) {
	if session := a.getSession(user.TelegramID); session != nil && session.Client != nil {
		session.Subgroup = user.Subgroup
		session.ShowAllSubgroups = user.ShowAllSubgroups
		if session.Halls == nil {
			halls, err := a.loadLectureHalls(ctx, session.Client, user.GroupID)
			if err != nil {
				slog.Warn("lecture halls load", "error", err)
				halls = make(ktk.LectureHallMap)
			}
			session.Halls = halls
		}
		if session.CallPresets == nil {
			session.CallPresets = a.loadCallPresets(ctx, session.Client)
		}
		if session.AbsenceMarks == nil {
			session.AbsenceMarks = a.loadAbsenceMarks(ctx, session.Client)
		}
		if user.PasswordLegacy {
			a.migrateLegacyPassword(user)
		}
		a.setSession(user.TelegramID, session)
		return session, nil
	}

	client, err := a.authClient(ctx, user.Login, user.Password, user.GroupID)
	if err != nil {
		return nil, err
	}
	if user.PasswordLegacy {
		a.migrateLegacyPassword(user)
	}

	halls, err := a.loadLectureHalls(ctx, client, user.GroupID)
	if err != nil {
		slog.Warn("lecture halls load", "error", err)
		halls = make(ktk.LectureHallMap)
	}

	callPresets := a.loadCallPresets(ctx, client)
	absenceMarks := a.loadAbsenceMarks(ctx, client)

	session := &Session{
		Client:           client,
		Halls:            halls,
		CallPresets:      callPresets,
		AbsenceMarks:     absenceMarks,
		CurrentIndex:     0,
		Subgroup:         user.Subgroup,
		ShowAllSubgroups: user.ShowAllSubgroups,
	}

	a.setSession(user.TelegramID, session)
	return session, nil
}

func (a *App) migrateLegacyPassword(user storage.User) {
	if err := a.storage.SaveUser(user); err != nil {
		slog.Error("legacy password migration", "error", err)
	}
}

func (a *App) authClient(ctx context.Context, login, password string, groupID int) (*ktk.Client, error) {
	endpoints := a.cachedEndpoints()
	client, err := ktk.NewClient(
		a.cfg.BaseURL,
		ktk.WithDeviceName(a.cfg.KTKDeviceName),
		ktk.WithScheduleDebug(a.cfg.KTKDebugSchedule),
		ktk.WithEndpoints(endpoints),
	)
	if err != nil {
		return nil, err
	}

	requestCtx, cancel := context.WithTimeout(ctx, 20*time.Second)
	defer cancel()

	if err := client.SignIn(requestCtx, login, password); err != nil {
		return nil, err
	}

	weekMillis := ktk.WeekStartMillis(time.Now(), a.location)
	if hasScheduleEndpoint(endpoints) {
		return client, nil
	}

	if err := client.RefreshEndpoints(requestCtx, groupID, weekMillis); err != nil {
		slog.Warn("endpoint discovery", "error", err)
		return client, nil
	}
	a.cacheEndpoints(client.Endpoints())

	return client, nil
}

func (a *App) refreshSessionSchedule(ctx context.Context, user storage.User, session *Session, targetDate time.Time) ([]ktk.ScheduleDay, int, error) {
	weekStart := ktk.WeekStart(targetDate, a.location)
	days, err := a.loadSchedule(ctx, session.Client, user.GroupID, weekStart)
	if err != nil {
		return nil, 0, err
	}

	displayDays := ktk.FilterScheduleDays(days, user.Subgroup, user.ShowAllSubgroups)
	if len(displayDays) == 0 {
		return displayDays, 0, nil
	}

	currentIndex := ktk.FindDateIndex(displayDays, targetDate, a.location)
	session.Schedule = displayDays
	session.CurrentIndex = currentIndex
	session.WeekStart = weekStart
	session.WeekSelectOffset = 0
	session.Subgroup = user.Subgroup
	session.ShowAllSubgroups = user.ShowAllSubgroups
	a.setSession(user.TelegramID, session)

	return displayDays, currentIndex, nil
}

func (a *App) loadScheduleForCallback(ctx context.Context, bot *telegram.Bot, chatID int64, messageID int, session *Session, targetDate time.Time) {
	user, err := a.storage.GetUser(chatID)
	if err != nil {
		a.send(ctx, chatID, "Ошибка базы данных: "+err.Error())
		return
	}
	if user == nil {
		a.send(ctx, chatID, "Сначала авторизуйся:\n/login логин пароль")
		return
	}

	days, _, err := a.refreshSessionSchedule(ctx, *user, session, targetDate)
	if err != nil {
		a.send(ctx, chatID, "Не удалось получить расписание: "+err.Error())
		return
	}
	if len(days) == 0 {
		a.send(ctx, chatID, "Расписание пустое. Попробуй другую неделю.")
		return
	}

	if targetDate.In(a.location).Format(time.DateOnly) == time.Now().In(a.location).Format(time.DateOnly) && !ktk.IsSchoolDay(days, targetDate, a.location) {
		a.editNonSchoolDayMessage(ctx, bot, chatID, messageID, session, targetDate)
		return
	}

	a.editScheduleMessage(ctx, bot, chatID, messageID, session)
}

func (a *App) loadSchedule(ctx context.Context, client *ktk.Client, groupID int, weekStart time.Time) ([]ktk.ScheduleDay, error) {
	requestCtx, cancel := context.WithTimeout(ctx, 20*time.Second)
	defer cancel()

	weekMillis := ktk.WeekStartMillis(weekStart, a.location)
	days, err := client.GetSchedule(requestCtx, groupID, weekMillis)
	if err == nil {
		a.cacheEndpoints(client.Endpoints())
	}
	return days, err
}

func (a *App) loadCallPresets(ctx context.Context, client *ktk.Client) ktk.CallPresetMap {
	requestCtx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()

	presets, err := client.GetCallPresets(requestCtx)
	if err != nil {
		slog.Warn("call presets load", "error", err)
		return nil
	}
	return presets
}

func (a *App) loadAbsenceMarks(ctx context.Context, client *ktk.Client) []ktk.AbsenceMark {
	requestCtx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()

	marks, err := client.GetAbsenceMarks(requestCtx)
	if err != nil {
		slog.Warn("absence marks load", "error", err)
		return nil
	}
	return marks
}

func (a *App) loadLectureHalls(ctx context.Context, client *ktk.Client, groupID int) (ktk.LectureHallMap, error) {
	requestCtx, cancel := context.WithTimeout(ctx, 20*time.Second)
	defer cancel()

	weekMillis := ktk.WeekStartMillis(time.Now(), a.location)
	halls, err := client.GetLectureHalls(requestCtx, groupID, weekMillis)
	if err == nil {
		a.cacheEndpoints(client.Endpoints())
	}
	return halls, err
}

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

	for _, user := range users {
		session, err := a.ensureSession(ctx, user)
		if err != nil {
			a.send(ctx, user.TelegramID, "Не удалось обновить утреннее расписание: "+err.Error())
			continue
		}

		displayDays, index, err := a.refreshSessionSchedule(ctx, user, session, time.Now())
		if err != nil {
			a.send(ctx, user.TelegramID, "Не удалось получить утреннее расписание: "+err.Error())
			continue
		}
		if len(displayDays) == 0 {
			a.send(ctx, user.TelegramID, "Доброе утро. Расписание на сегодня не найдено.")
			continue
		}
		if !ktk.IsSchoolDay(displayDays, time.Now(), a.location) {
			continue
		}

		if ktk.AllSubjectsRemote(displayDays[index]) {
			continue
		}

		text := "Доброе утро. Расписание на сегодня:\n\n" + a.formatScheduleDay(displayDays[index], session)
		a.sendMessage(ctx, &telegram.SendMessageParams{
			ChatID:      user.TelegramID,
			Text:        text,
			ReplyMarkup: tg.ScheduleKeyboard(displayDays, index, session.WeekStart, a.location),
		})
	}
}

func (a *App) send(ctx context.Context, chatID int64, text string) {
	a.sendMessage(ctx, &telegram.SendMessageParams{ChatID: chatID, Text: text})
}

func (a *App) sendMessage(ctx context.Context, params *telegram.SendMessageParams) {
	if _, err := a.bot.SendMessage(ctx, params); err != nil {
		slog.Error("send message", "error", err)
	}
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

func hasScheduleEndpoint(endpoints ktk.Endpoints) bool {
	return strings.TrimSpace(endpoints.SchedulePath) != ""
}

func (a *App) requireUser(ctx context.Context, msg *models.Message) (*storage.User, bool) {
	if msg == nil {
		return nil, false
	}
	user, err := a.storage.GetUser(msg.Chat.ID)
	if err != nil {
		a.send(ctx, msg.Chat.ID, "Ошибка базы данных: "+err.Error())
		return nil, false
	}
	if user == nil {
		a.send(ctx, msg.Chat.ID, "Сначала авторизуйся:\n/login логин пароль")
		return nil, false
	}
	return user, true
}

func (a *App) todayIndex(days []ktk.ScheduleDay) int {
	return ktk.FindDateIndex(days, time.Now(), a.location)
}

func (a *App) formatScheduleDay(day ktk.ScheduleDay, session *Session) string {
	return ktk.FormatScheduleDayWithOptions(day, session.Halls, ktk.FormatOptions{
		ShowSubgroupLabels: session.ShowAllSubgroups,
		CallPresets:        session.CallPresets,
		AbsenceMarks:       session.AbsenceMarks,
		Loc:                a.location,
		Now:                time.Now(),
	})
}

func isMessageNotModified(err error) bool {
	if !errors.Is(err, telegram.ErrorBadRequest) {
		return false
	}
	return strings.Contains(err.Error(), "message is not modified")
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
