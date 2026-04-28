package app

import (
	"context"
	"fmt"
	"log"
	"strconv"
	"strings"
	"time"

	tgbotapi "github.com/go-telegram-bot-api/telegram-bot-api/v5"

	"ktk-schedule/internal/config"
	"ktk-schedule/internal/ktk"
	"ktk-schedule/internal/storage"
	"ktk-schedule/internal/tg"
)

type App struct {
	cfg      config.Config
	bot      *tgbotapi.BotAPI
	storage  *storage.Storage
	location *time.Location
	sessions map[int64]*Session
}

type Session struct {
	Client       *ktk.Client
	Schedule     []ktk.ScheduleDay
	Halls        ktk.LectureHallMap
	CurrentIndex int
}

func New(cfg config.Config) (*App, error) {
	bot, err := tgbotapi.NewBotAPI(cfg.BotToken)
	if err != nil {
		return nil, err
	}

	store, err := storage.New(cfg.DatabasePath)
	if err != nil {
		return nil, err
	}

	location, err := time.LoadLocation(cfg.Timezone)
	if err != nil {
		return nil, err
	}

	return &App{
		cfg:      cfg,
		bot:      bot,
		storage:  store,
		location: location,
		sessions: make(map[int64]*Session),
	}, nil
}

func (a *App) Close() {
	if err := a.storage.Close(); err != nil {
		log.Println("storage close error:", err)
	}
}

func (a *App) Run() error {
	log.Printf("Bot started: @%s", a.bot.Self.UserName)

	go a.runNotifier()

	updateConfig := tgbotapi.NewUpdate(0)
	updateConfig.Timeout = 30

	for update := range a.bot.GetUpdatesChan(updateConfig) {
		a.handleUpdate(update)
	}

	return nil
}

func (a *App) handleUpdate(update tgbotapi.Update) {
	if update.CallbackQuery != nil {
		a.handleCallback(update.CallbackQuery)
		return
	}

	if update.Message == nil {
		return
	}

	message := update.Message
	chatID := message.Chat.ID

	switch message.Command() {
	case "start":
		a.send(chatID, "Привет! Я ktk-schedule\n\nКоманды:\n/login логин пароль\n/schedule\n/group 269\n/notify_on\n/notify_off")

	case "login":
		a.handleLogin(chatID, message.CommandArguments())

	case "schedule":
		a.handleSchedule(chatID)

	case "group":
		a.handleGroup(chatID, message.CommandArguments())

	case "notify_on":
		a.handleNotify(chatID, true)

	case "notify_off":
		a.handleNotify(chatID, false)

	default:
		a.send(chatID, "Неизвестная команда. Напиши /start")
	}
}

func (a *App) handleLogin(chatID int64, rawArgs string) {
	args := strings.Fields(rawArgs)
	if len(args) != 2 {
		a.send(chatID, "Используй:\n/login логин пароль")
		return
	}

	login := args[0]
	password := args[1]

	client, err := a.authClient(login, password)
	if err != nil {
		a.send(chatID, "Не удалось войти: "+err.Error())
		return
	}

	user := storage.User{
		TelegramID: chatID,
		Login:      login,
		Password:   password,
		GroupID:    a.cfg.DefaultGroup,
		Notify:     false,
	}

	if err := a.storage.SaveUser(user); err != nil {
		a.send(chatID, "Не удалось сохранить пользователя: "+err.Error())
		return
	}

	halls, err := a.loadLectureHalls(client)
	if err != nil {
		log.Println("lecture halls load error:", err)
		halls = make(ktk.LectureHallMap)
	}

	a.sessions[chatID] = &Session{
		Client:       client,
		Halls:        halls,
		CurrentIndex: 0,
	}

	a.send(chatID, fmt.Sprintf("Авторизация успешна.\nГруппа: %d\n\nТеперь напиши /schedule", user.GroupID))
}

func (a *App) handleSchedule(chatID int64) {
	user, err := a.storage.GetUser(chatID)
	if err != nil {
		a.send(chatID, "Ошибка базы данных: "+err.Error())
		return
	}
	if user == nil {
		a.send(chatID, "Сначала авторизуйся:\n/login логин пароль")
		return
	}

	session, err := a.ensureSession(*user)
	if err != nil {
		a.send(chatID, "Не удалось авторизоваться: "+err.Error())
		return
	}

	days, err := a.loadSchedule(session.Client, user.GroupID)
	if err != nil {
		a.send(chatID, "Не удалось получить расписание: "+err.Error())
		return
	}

	currentIndex := ktk.FindTodayIndex(days)

	session.Schedule = days
	session.CurrentIndex = currentIndex
	a.sessions[chatID] = session

	msg := tgbotapi.NewMessage(chatID, ktk.FormatScheduleDay(days[currentIndex], session.Halls))
	msg.ReplyMarkup = tg.ScheduleKeyboard(days, currentIndex)
	a.sendConfig(msg)
}

func (a *App) handleGroup(chatID int64, rawArgs string) {
	user, err := a.storage.GetUser(chatID)
	if err != nil {
		a.send(chatID, "Ошибка базы данных: "+err.Error())
		return
	}
	if user == nil {
		a.send(chatID, "Сначала авторизуйся:\n/login логин пароль")
		return
	}

	groupID, err := strconv.Atoi(strings.TrimSpace(rawArgs))
	if err != nil || groupID <= 0 {
		a.send(chatID, "Используй:\n/group 269")
		return
	}

	if err := a.storage.SetGroup(chatID, groupID); err != nil {
		a.send(chatID, "Не удалось сохранить группу: "+err.Error())
		return
	}

	a.send(chatID, fmt.Sprintf("Группа изменена на %d.\nТеперь напиши /schedule", groupID))
}

func (a *App) handleNotify(chatID int64, enabled bool) {
	user, err := a.storage.GetUser(chatID)
	if err != nil {
		a.send(chatID, "Ошибка базы данных: "+err.Error())
		return
	}
	if user == nil {
		a.send(chatID, "Сначала авторизуйся:\n/login логин пароль")
		return
	}

	if err := a.storage.SetNotify(chatID, enabled); err != nil {
		a.send(chatID, "Не удалось сохранить настройку: "+err.Error())
		return
	}

	if enabled {
		a.send(chatID, "Утреннее расписание включено.")
	} else {
		a.send(chatID, "Утреннее расписание выключено.")
	}
}

func (a *App) handleCallback(callback *tgbotapi.CallbackQuery) {
	chatID := callback.Message.Chat.ID

	_, _ = a.bot.Request(tgbotapi.NewCallback(callback.ID, ""))

	session := a.sessions[chatID]
	if session == nil || len(session.Schedule) == 0 {
		a.send(chatID, "Расписание устарело. Напиши /schedule ещё раз.")
		return
	}

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
		session.CurrentIndex = ktk.FindTodayIndex(session.Schedule)

	case strings.HasPrefix(callback.Data, "schedule:day:"):
		value := strings.TrimPrefix(callback.Data, "schedule:day:")
		index, err := strconv.Atoi(value)
		if err != nil || index < 0 || index >= len(session.Schedule) {
			return
		}
		session.CurrentIndex = index

	default:
		return
	}

	day := session.Schedule[session.CurrentIndex]

	edit := tgbotapi.NewEditMessageText(chatID, callback.Message.MessageID, ktk.FormatScheduleDay(day, session.Halls))
	edit.ReplyMarkup = tg.ScheduleKeyboard(session.Schedule, session.CurrentIndex)

	if _, err := a.bot.Send(edit); err != nil {
		log.Println("edit message error:", err)
	}
}

func (a *App) ensureSession(user storage.User) (*Session, error) {
	if session := a.sessions[user.TelegramID]; session != nil && session.Client != nil {
		if session.Halls == nil {
			halls, err := a.loadLectureHalls(session.Client)
			if err != nil {
				log.Println("lecture halls load error:", err)
				halls = make(ktk.LectureHallMap)
			}
			session.Halls = halls
		}

		return session, nil
	}

	client, err := a.authClient(user.Login, user.Password)
	if err != nil {
		return nil, err
	}

	halls, err := a.loadLectureHalls(client)
	if err != nil {
		log.Println("lecture halls load error:", err)
		halls = make(ktk.LectureHallMap)
	}

	session := &Session{
		Client:       client,
		Halls:        halls,
		CurrentIndex: 0,
	}

	a.sessions[user.TelegramID] = session
	return session, nil
}

func (a *App) authClient(login, password string) (*ktk.Client, error) {
	client, err := ktk.NewClient(a.cfg.BaseURL)
	if err != nil {
		return nil, err
	}

	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()

	if err := client.SignIn(ctx, login, password); err != nil {
		return nil, err
	}

	return client, nil
}

func (a *App) loadSchedule(client *ktk.Client, groupID int) ([]ktk.ScheduleDay, error) {
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()

	weekMillis := ktk.WeekStartMillis(time.Now(), a.location)
	return client.GetSchedule(ctx, groupID, weekMillis)
}

func (a *App) loadLectureHalls(client *ktk.Client) (ktk.LectureHallMap, error) {
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()

	return client.GetLectureHalls(ctx)
}

func (a *App) runNotifier() {
	ticker := time.NewTicker(time.Minute)
	defer ticker.Stop()

	lastRunDate := ""

	for range ticker.C {
		now := time.Now().In(a.location)
		currentTime := now.Format("15:04")
		currentDate := now.Format("2006-01-02")

		if currentTime != a.cfg.NotifyTime || lastRunDate == currentDate {
			continue
		}

		lastRunDate = currentDate
		a.sendDailySchedules()
	}
}

func (a *App) sendDailySchedules() {
	users, err := a.storage.ListNotifyUsers()
	if err != nil {
		log.Println("list notify users error:", err)
		return
	}

	for _, user := range users {
		session, err := a.ensureSession(user)
		if err != nil {
			a.send(user.TelegramID, "Не удалось обновить утреннее расписание: "+err.Error())
			continue
		}

		days, err := a.loadSchedule(session.Client, user.GroupID)
		if err != nil {
			a.send(user.TelegramID, "Не удалось получить утреннее расписание: "+err.Error())
			continue
		}

		index := ktk.FindTodayIndex(days)
		text := "Доброе утро. Расписание на сегодня:\n\n" + ktk.FormatScheduleDay(days[index], session.Halls)

		msg := tgbotapi.NewMessage(user.TelegramID, text)
		msg.ReplyMarkup = tg.ScheduleKeyboard(days, index)
		a.sendConfig(msg)

		session.Schedule = days
		session.CurrentIndex = index
		a.sessions[user.TelegramID] = session
	}
}

func (a *App) send(chatID int64, text string) {
	a.sendConfig(tgbotapi.NewMessage(chatID, text))
}

func (a *App) sendConfig(config tgbotapi.MessageConfig) {
	if _, err := a.bot.Send(config); err != nil {
		log.Println("send message error:", err)
	}
}
