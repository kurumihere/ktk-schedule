package app

import (
	"context"
	"fmt"
	"log"
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
	weekSelectPageSize = 5
	announceSendDelay  = 50 * time.Millisecond
)

type App struct {
	cfg      config.Config
	bot      *telegram.Bot
	storage  *storage.Storage
	location *time.Location

	endpointsMu sync.RWMutex
	endpoints   ktk.Endpoints

	sessionsMu sync.RWMutex
	sessions   map[int64]*Session
}

type Session struct {
	Client           *ktk.Client
	Schedule         []ktk.ScheduleDay
	Halls            ktk.LectureHallMap
	CurrentIndex     int
	WeekStart        time.Time
	WeekSelectOffset int
	Subgroup         string
	ShowAllSubgroups bool
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
			BranchID:        cfg.KTKBranchID,
		},
		sessions: make(map[int64]*Session),
	}

	bot, err := telegram.New(
		cfg.BotToken,
		telegram.WithAllowedUpdates(telegram.AllowedUpdates{
			models.AllowedUpdateMessage,
			models.AllowedUpdateCallbackQuery,
		}),
		telegram.WithDefaultHandler(app.handleDefault),
		telegram.WithErrorsHandler(func(err error) {
			log.Println("telegram error:", err)
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
	a.bot.RegisterHandler(telegram.HandlerTypeCallbackQueryData, "schedule:", telegram.MatchTypePrefix, a.handleCallback)
}

func (a *App) Close() {
	if err := a.storage.Close(); err != nil {
		log.Println("storage close error:", err)
	}
}

func (a *App) Run(ctx context.Context) error {
	if me, err := a.bot.GetMe(ctx); err == nil {
		log.Printf("Bot started: @%s", me.Username)
	} else {
		log.Println("Bot started")
		log.Println("get me error:", err)
	}

	go a.runNotifier(ctx)
	a.bot.Start(ctx)
	return nil
}

func (a *App) getSession(telegramID int64) *Session {
	a.sessionsMu.RLock()
	defer a.sessionsMu.RUnlock()

	session := a.sessions[telegramID]
	if session == nil {
		return nil
	}

	copy := *session
	return &copy
}

func (a *App) setSession(telegramID int64, session *Session) {
	if session == nil {
		return
	}

	copy := *session
	a.sessionsMu.Lock()
	defer a.sessionsMu.Unlock()
	a.sessions[telegramID] = &copy
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
		log.Println("lecture halls load error:", err)
		halls = make(ktk.LectureHallMap)
	}

	a.setSession(chatID, &Session{
		Client:           client,
		Halls:            halls,
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
	user, err := a.storage.GetUser(chatID)
	if err != nil {
		a.send(ctx, chatID, "Ошибка базы данных: "+err.Error())
		return
	}
	if user == nil {
		a.send(ctx, chatID, "Сначала авторизуйся:\n/login логин пароль")
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

	a.sendMessage(ctx, &telegram.SendMessageParams{
		ChatID:      chatID,
		Text:        a.formatScheduleDay(displayDays[currentIndex], session),
		ReplyMarkup: tg.ScheduleKeyboard(displayDays, currentIndex, session.WeekStart, a.location),
	})
}

func (a *App) handleGroup(ctx context.Context, _ *telegram.Bot, update *models.Update) {
	if update.Message == nil {
		return
	}

	chatID := update.Message.Chat.ID
	user, err := a.storage.GetUser(chatID)
	if err != nil {
		a.send(ctx, chatID, "Ошибка базы данных: "+err.Error())
		return
	}
	if user == nil {
		a.send(ctx, chatID, "Сначала авторизуйся:\n/login логин пароль")
		return
	}

	groupID, err := strconv.Atoi(strings.TrimSpace(commandArgs(update.Message.Text)))
	if err != nil || groupID <= 0 {
		a.send(ctx, chatID, "Используй:\n/group 269")
		return
	}

	if err := a.storage.SetGroup(chatID, groupID); err != nil {
		a.send(ctx, chatID, "Не удалось сохранить группу: "+err.Error())
		return
	}

	a.send(ctx, chatID, fmt.Sprintf("Группа изменена на %d.\nТеперь напиши /schedule", groupID))
}

func (a *App) handleSubgroup(ctx context.Context, _ *telegram.Bot, update *models.Update) {
	if update.Message == nil {
		return
	}

	chatID := update.Message.Chat.ID
	user, err := a.storage.GetUser(chatID)
	if err != nil {
		a.send(ctx, chatID, "Ошибка базы данных: "+err.Error())
		return
	}
	if user == nil {
		a.send(ctx, chatID, "Сначала авторизуйся:\n/login логин пароль")
		return
	}

	subgroup, ok := ktk.ParsePersonalSubgroup(commandArgs(update.Message.Text))
	if !ok {
		a.send(ctx, chatID, "Используй:\n/subgroup 1\nили\n/subgroup 2")
		return
	}

	if err := a.storage.SetSubgroup(chatID, subgroup); err != nil {
		a.send(ctx, chatID, "Не удалось сохранить подгруппу: "+err.Error())
		return
	}
	if session := a.getSession(chatID); session != nil {
		session.Subgroup = subgroup
		session.ShowAllSubgroups = false
		a.setSession(chatID, session)
	}

	a.send(ctx, chatID, "Подгруппа изменена: "+ktk.SubgroupLabel(subgroup)+".\nТеперь напиши /schedule")
}

func (a *App) handleSubgroupsOn(ctx context.Context, _ *telegram.Bot, update *models.Update) {
	a.handleSubgroupsMode(ctx, update, true)
}

func (a *App) handleSubgroupsOff(ctx context.Context, _ *telegram.Bot, update *models.Update) {
	a.handleSubgroupsMode(ctx, update, false)
}

func (a *App) handleSubgroupsMode(ctx context.Context, update *models.Update, enabled bool) {
	if update.Message == nil {
		return
	}

	chatID := update.Message.Chat.ID
	user, err := a.storage.GetUser(chatID)
	if err != nil {
		a.send(ctx, chatID, "Ошибка базы данных: "+err.Error())
		return
	}
	if user == nil {
		a.send(ctx, chatID, "Сначала авторизуйся:\n/login логин пароль")
		return
	}

	if err := a.storage.SetShowAllSubgroups(chatID, enabled); err != nil {
		a.send(ctx, chatID, "Не удалось сохранить режим подгрупп: "+err.Error())
		return
	}
	if session := a.getSession(chatID); session != nil {
		session.ShowAllSubgroups = enabled
		a.setSession(chatID, session)
	}

	if enabled {
		a.send(ctx, chatID, "Теперь показываю обе подгруппы.\nНапиши /schedule")
	} else {
		a.send(ctx, chatID, "Теперь показываю только твою подгруппу: "+ktk.SubgroupLabel(user.Subgroup)+".\nНапиши /schedule")
	}
}

func (a *App) handleNotifyOn(ctx context.Context, _ *telegram.Bot, update *models.Update) {
	a.handleNotify(ctx, update, true)
}

func (a *App) handleNotifyOff(ctx context.Context, _ *telegram.Bot, update *models.Update) {
	a.handleNotify(ctx, update, false)
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
			log.Printf("announcement delivery error chat_id=%d: %v", recipient, err)
			continue
		}
		sent++
	}

	return sent, failed
}

func (a *App) handleNotify(ctx context.Context, update *models.Update, enabled bool) {
	if update.Message == nil {
		return
	}

	chatID := update.Message.Chat.ID
	user, err := a.storage.GetUser(chatID)
	if err != nil {
		a.send(ctx, chatID, "Ошибка базы данных: "+err.Error())
		return
	}
	if user == nil {
		a.send(ctx, chatID, "Сначала авторизуйся:\n/login логин пароль")
		return
	}

	if err := a.storage.SetNotify(chatID, enabled); err != nil {
		a.send(ctx, chatID, "Не удалось сохранить настройку: "+err.Error())
		return
	}

	if enabled {
		a.send(ctx, chatID, "Утреннее расписание включено.")
	} else {
		a.send(ctx, chatID, "Утреннее расписание выключено.")
	}
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
		a.send(ctx, chatID, "Расписание устарело. Напиши /schedule ещё раз.")
		return
	}
	if session.WeekStart.IsZero() {
		session.WeekStart = ktk.WeekStart(time.Now(), a.location)
	}

	data := callback.Data
	oldIndex := session.CurrentIndex
	switch {
	case data == "schedule:week:select":
		session.WeekSelectOffset = 0
		a.setSession(chatID, session)
		a.editWeekSelectMessage(ctx, bot, chatID, message.ID, session)
		return
	case strings.HasPrefix(data, "schedule:week:page:"):
		delta, err := strconv.Atoi(strings.TrimPrefix(data, "schedule:week:page:"))
		if err != nil {
			return
		}
		session.WeekSelectOffset += delta * weekSelectPageSize
		a.setSession(chatID, session)
		a.editWeekSelectMessage(ctx, bot, chatID, message.ID, session)
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
		weekMillis, err := strconv.ParseInt(strings.TrimPrefix(data, "schedule:week:open:"), 10, 64)
		if err != nil {
			return
		}
		a.loadScheduleForCallback(ctx, bot, chatID, message.ID, session, ktk.WeekStartFromMillis(weekMillis, a.location))
		return
	case data == "schedule:prev":
		if session.CurrentIndex > 0 {
			session.CurrentIndex--
		}
	case data == "schedule:next":
		if session.CurrentIndex < len(session.Schedule)-1 {
			session.CurrentIndex++
		}
	case data == "schedule:today":
		todayWeekStart := ktk.WeekStart(time.Now(), a.location)
		if !session.WeekStart.Equal(todayWeekStart) {
			a.loadScheduleForCallback(ctx, bot, chatID, message.ID, session, time.Now())
			return
		}
		session.CurrentIndex = a.todayIndex(session.Schedule)
	case strings.HasPrefix(data, "schedule:day:"):
		index, err := strconv.Atoi(strings.TrimPrefix(data, "schedule:day:"))
		if err != nil || index < 0 || index >= len(session.Schedule) {
			return
		}
		session.CurrentIndex = index
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
		log.Println("edit message error:", err)
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
		log.Println("edit week select message error:", err)
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
				log.Println("lecture halls load error:", err)
				halls = make(ktk.LectureHallMap)
			}
			session.Halls = halls
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
		log.Println("lecture halls load error:", err)
		halls = make(ktk.LectureHallMap)
	}

	session := &Session{
		Client:           client,
		Halls:            halls,
		CurrentIndex:     0,
		Subgroup:         user.Subgroup,
		ShowAllSubgroups: user.ShowAllSubgroups,
	}

	a.setSession(user.TelegramID, session)
	return session, nil
}

func (a *App) migrateLegacyPassword(user storage.User) {
	if err := a.storage.SaveUser(user); err != nil {
		log.Println("legacy password migration error:", err)
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
		log.Println("endpoint discovery error:", err)
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

func (a *App) sendDailySchedules(ctx context.Context) {
	users, err := a.storage.ListNotifyUsers()
	if err != nil {
		log.Println("list notify users error:", err)
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
		log.Println("send message error:", err)
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

func (a *App) todayIndex(days []ktk.ScheduleDay) int {
	return ktk.FindDateIndex(days, time.Now(), a.location)
}

func (a *App) formatScheduleDay(day ktk.ScheduleDay, session *Session) string {
	return ktk.FormatScheduleDayWithOptions(day, session.Halls, ktk.FormatOptions{
		ShowSubgroupLabels: session.ShowAllSubgroups,
	})
}

func isMessageNotModified(err error) bool {
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
