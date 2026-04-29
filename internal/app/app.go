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

const helpText = "Привет! Я ktk-schedule\n\nКоманды:\n/login логин пароль\n/schedule\n/group 269\n/notify_on\n/notify_off"

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
	Client       *ktk.Client
	Schedule     []ktk.ScheduleDay
	Halls        ktk.LectureHallMap
	CurrentIndex int
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
		return nil, err
	}

	app.bot = bot
	app.registerHandlers()
	return app, nil
}

func (a *App) registerHandlers() {
	a.bot.RegisterHandler(telegram.HandlerTypeMessageText, "start", telegram.MatchTypeCommandStartOnly, a.handleStart)
	a.bot.RegisterHandler(telegram.HandlerTypeMessageText, "login", telegram.MatchTypeCommandStartOnly, a.handleLogin)
	a.bot.RegisterHandler(telegram.HandlerTypeMessageText, "schedule", telegram.MatchTypeCommandStartOnly, a.handleSchedule)
	a.bot.RegisterHandler(telegram.HandlerTypeMessageText, "group", telegram.MatchTypeCommandStartOnly, a.handleGroup)
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
	user := storage.User{
		TelegramID: chatID,
		Login:      login,
		Password:   password,
		GroupID:    a.cfg.DefaultGroup,
		Notify:     false,
	}

	client, err := a.authClient(ctx, login, password, user.GroupID)
	if err != nil {
		a.send(ctx, chatID, "Не удалось войти: "+err.Error())
		return
	}

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
		Client:       client,
		Halls:        halls,
		CurrentIndex: 0,
	})

	a.send(ctx, chatID, fmt.Sprintf("Авторизация успешна.\nГруппа: %d\n\nТеперь напиши /schedule", user.GroupID))
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

	days, err := a.loadSchedule(ctx, session.Client, user.GroupID)
	if err != nil {
		a.send(ctx, chatID, "Не удалось получить расписание: "+err.Error())
		return
	}
	if len(days) == 0 {
		a.send(ctx, chatID, "Расписание пустое. Попробуй позже.")
		return
	}

	currentIndex := a.todayIndex(days)
	session.Schedule = days
	session.CurrentIndex = currentIndex
	a.setSession(chatID, session)

	a.sendMessage(ctx, &telegram.SendMessageParams{
		ChatID:      chatID,
		Text:        ktk.FormatScheduleDay(days[currentIndex], session.Halls),
		ReplyMarkup: tg.ScheduleKeyboard(days, currentIndex),
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

func (a *App) handleNotifyOn(ctx context.Context, _ *telegram.Bot, update *models.Update) {
	a.handleNotify(ctx, update, true)
}

func (a *App) handleNotifyOff(ctx context.Context, _ *telegram.Bot, update *models.Update) {
	a.handleNotify(ctx, update, false)
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
	if session == nil || len(session.Schedule) == 0 {
		a.send(ctx, chatID, "Расписание устарело. Напиши /schedule ещё раз.")
		return
	}

	oldIndex := session.CurrentIndex
	switch {
	case callback.Data == "schedule:prev":
		if session.CurrentIndex > 0 {
			session.CurrentIndex--
		}
	case callback.Data == "schedule:next":
		if session.CurrentIndex < len(session.Schedule)-1 {
			session.CurrentIndex++
		}
	case callback.Data == "schedule:today":
		session.CurrentIndex = a.todayIndex(session.Schedule)
	case strings.HasPrefix(callback.Data, "schedule:day:"):
		index, err := strconv.Atoi(strings.TrimPrefix(callback.Data, "schedule:day:"))
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

	day := session.Schedule[session.CurrentIndex]
	_, err := bot.EditMessageText(ctx, &telegram.EditMessageTextParams{
		ChatID:      chatID,
		MessageID:   message.ID,
		Text:        ktk.FormatScheduleDay(day, session.Halls),
		ReplyMarkup: tg.ScheduleKeyboard(session.Schedule, session.CurrentIndex),
	})
	if err != nil {
		if isMessageNotModified(err) {
			return
		}
		log.Println("edit message error:", err)
	}
}

func (a *App) handleDefault(ctx context.Context, _ *telegram.Bot, update *models.Update) {
	if update.Message == nil || update.Message.Text == "" {
		return
	}
	a.send(ctx, update.Message.Chat.ID, "Неизвестная команда. Напиши /start")
}

func (a *App) ensureSession(ctx context.Context, user storage.User) (*Session, error) {
	if session := a.getSession(user.TelegramID); session != nil && session.Client != nil {
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
		Client:       client,
		Halls:        halls,
		CurrentIndex: 0,
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

func (a *App) loadSchedule(ctx context.Context, client *ktk.Client, groupID int) ([]ktk.ScheduleDay, error) {
	requestCtx, cancel := context.WithTimeout(ctx, 20*time.Second)
	defer cancel()

	weekMillis := ktk.WeekStartMillis(time.Now(), a.location)
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

		days, err := a.loadSchedule(ctx, session.Client, user.GroupID)
		if err != nil {
			a.send(ctx, user.TelegramID, "Не удалось получить утреннее расписание: "+err.Error())
			continue
		}
		if len(days) == 0 {
			a.send(ctx, user.TelegramID, "Доброе утро. Расписание на сегодня не найдено.")
			continue
		}

		index := a.todayIndex(days)
		text := "Доброе утро. Расписание на сегодня:\n\n" + ktk.FormatScheduleDay(days[index], session.Halls)
		a.sendMessage(ctx, &telegram.SendMessageParams{
			ChatID:      user.TelegramID,
			Text:        text,
			ReplyMarkup: tg.ScheduleKeyboard(days, index),
		})

		session.Schedule = days
		session.CurrentIndex = index
		a.setSession(user.TelegramID, session)
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

func isMessageNotModified(err error) bool {
	return strings.Contains(err.Error(), "message is not modified")
}
