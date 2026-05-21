package app

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"net/http"
	_ "net/http/pprof"
	"strings"
	"sync"
	"time"

	telegram "github.com/go-telegram/bot"
	"github.com/go-telegram/bot/models"

	"ktk-schedule/internal/config"
	"ktk-schedule/internal/credentials"
	"ktk-schedule/internal/ktk"
	"ktk-schedule/internal/storage"
)

const (
	weekSelectPageSize     = 5
	announceSendDelay      = 50 * time.Millisecond
	sessionMaxAge          = 30 * time.Minute
	sessionCleanupInterval = 10 * time.Minute
	notifyConcurrency      = 5
	announceConcurrency    = 10
)

type App struct {
	cfg      config.Config
	bot      *telegram.Bot
	storage  *storage.Storage
	location *time.Location

	endpointsMu sync.RWMutex
	endpoints   ktk.Endpoints

	sessions       sync.Map // telegramID → *atomic.Pointer[Session]
	healthServer   *http.Server
	rateLimiter    *rateLimiter
	loginLimiter   *rateLimiter
	circuitBreaker *circuitBreaker
	scheduleCache  *scheduleCache
	startedAt      time.Time
	activeHandlers sync.WaitGroup

	botCtx    context.Context
	botCancel context.CancelFunc
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
		rateLimiter:    newRateLimiter(),
		loginLimiter:   newLoginRateLimiter(),
		circuitBreaker: newCircuitBreaker(5, 30*time.Second),
		scheduleCache:  newScheduleCache(),
		startedAt:      time.Now(),
	}

	mux := http.NewServeMux()
	mux.HandleFunc("/health", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusOK)
		fmt.Fprint(w, `{"status":"ok"}`)
	})
	mux.HandleFunc("/health/extended", func(w http.ResponseWriter, r *http.Request) {
		activeSessions := app.sessionCount()

		totalUsers, _ := app.storage.CountUsers()
		notifyUsers, _ := app.storage.CountNotifyUsers()

		resp := map[string]any{
			"status":          "ok",
			"uptime":          formatUptime(time.Since(app.startedAt)),
			"total_users":     totalUsers,
			"notify_users":    notifyUsers,
			"active_sessions": activeSessions,
			"timezone":        app.cfg.Timezone,
		}

		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusOK)
		_ = json.NewEncoder(w).Encode(resp)
	})
	mux.HandleFunc("/debug/pprof/", func(w http.ResponseWriter, r *http.Request) {
		http.DefaultServeMux.ServeHTTP(w, r)
	})
	app.healthServer = &http.Server{
		Addr:         ":" + cfg.HealthPort,
		Handler:      mux,
		ReadTimeout:  5 * time.Second,
		WriteTimeout: 5 * time.Second,
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

func (a *App) Close() {
	a.rateLimiter.Close()
	a.loginLimiter.Close()

	if a.botCancel != nil {
		a.botCancel()
	}

	slog.Info("shutting down, waiting for active handlers")
	done := make(chan struct{})
	go func() {
		a.activeHandlers.Wait()
		close(done)
	}()
	select {
	case <-done:
		slog.Info("all handlers completed")
	case <-time.After(5 * time.Second):
		slog.Warn("timeout waiting for handlers, forcing shutdown")
	}

	shutdownCtx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	if err := a.healthServer.Shutdown(shutdownCtx); err != nil {
		slog.Error("health server shutdown", "error", err)
	}

	if err := a.storage.Close(); err != nil {
		slog.Error("storage close", "error", err)
	}
	slog.Info("shutdown complete")
}

func (a *App) Run(ctx context.Context) error {
	a.botCtx, a.botCancel = context.WithCancel(ctx)

	if me, err := a.bot.GetMe(a.botCtx); err == nil {
		slog.Info("bot started", "username", me.Username)
	} else {
		slog.Warn("bot started", "error", err)
	}

	go a.runNotifier(a.botCtx)
	go a.sessionCleanupLoop(a.botCtx)
	go func() {
		if err := a.healthServer.ListenAndServe(); err != nil && err != http.ErrServerClosed {
			slog.Error("health server", "error", err)
		}
	}()
	a.bot.Start(a.botCtx)

	return nil
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

func (a *App) send(ctx context.Context, chatID int64, text string) {
	if err := sendMessageWithRetry(ctx, a.bot, &telegram.SendMessageParams{ChatID: chatID, Text: text}); err != nil {
		slog.Error("send message", "chat_id", chatID, "error", err)
	}
}

func (a *App) sendMessage(ctx context.Context, params *telegram.SendMessageParams) {
	if err := sendMessageWithRetry(ctx, a.bot, params); err != nil {
		slog.Error("send message", "chat_id", params.ChatID, "error", err)
	}
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
		AbsenceByDigit:     buildAbsenceByDigit(session.AbsenceMarks),
		Loc:                a.location,
		Now:                time.Now(),
	})
}

func buildAbsenceByDigit(marks []ktk.AbsenceMark) map[int]string {
	if marks == nil {
		return nil
	}
	m := make(map[int]string, len(marks))
	for _, am := range marks {
		m[am.Digit] = am.Caption
	}
	return m
}

func hasScheduleEndpoint(endpoints ktk.Endpoints) bool {
	return strings.TrimSpace(endpoints.SchedulePath) != ""
}

func isMessageNotModified(err error) bool {
	if !errors.Is(err, telegram.ErrorBadRequest) {
		return false
	}
	return strings.HasPrefix(extractTelegramDescription(err), "message is not modified")
}

func extractTelegramDescription(err error) string {
	msg := err.Error()
	if idx := strings.Index(msg, ", "); idx >= 0 {
		return msg[idx+2:]
	}
	return msg
}
