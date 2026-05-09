package config

import (
	"errors"
	"log/slog"
	"os"
	"strconv"
	"strings"

	"github.com/joho/godotenv"
)

type Config struct {
	BotToken           string
	BaseURL            string
	DatabasePath       string
	CredentialsSecret  string
	KTKSignInPath      string
	KTKSchedulePath    string
	KTKLectureHallPath string
	KTKBranchID        string
	KTKDeviceName      string
	KTKDebugSchedule   bool
	KTKCallPresetPath  string
	DefaultGroup       int
	DefaultSubgroup    string
	OwnerTelegramID    int64
	NotifyTime         string
	Timezone           string
	LogLevel           slog.Level
	HealthPort         string
}

func Load() (Config, error) {
	_ = godotenv.Load()

	defaultGroup, err := strconv.Atoi(getenv("DEFAULT_GROUP_ID", "269"))
	if err != nil {
		return Config{}, err
	}
	debugSchedule, err := getenvBool("KTK_DEBUG_SCHEDULE", false)
	if err != nil {
		return Config{}, err
	}
	ownerTelegramID, err := getenvInt64("OWNER_TELEGRAM_ID", 0)
	if err != nil {
		return Config{}, err
	}
	if ownerTelegramID < 0 {
		return Config{}, errors.New("OWNER_TELEGRAM_ID must be positive")
	}

	logLevel := parseLogLevel(os.Getenv("LOG_LEVEL"))

	cfg := Config{
		BotToken:           strings.TrimSpace(os.Getenv("BOT_TOKEN")),
		BaseURL:            getenv("KTK_BASE_URL", "https://workspace.ktk-45.ru"),
		DatabasePath:       getenv("DATABASE_PATH", "ktk-schedule.db"),
		CredentialsSecret:  strings.TrimSpace(os.Getenv("CREDENTIALS_SECRET")),
		LogLevel:           logLevel,
		HealthPort:         getenv("HEALTH_PORT", "8080"),
		KTKSignInPath:      getenv("KTK_SIGN_IN_PATH", "/sign-in"),
		KTKSchedulePath:    strings.TrimSpace(os.Getenv("KTK_SCHEDULE_PATH")),
		KTKLectureHallPath: strings.TrimSpace(os.Getenv("KTK_LECTURE_HALL_PATH")),
		KTKBranchID:        strings.TrimSpace(os.Getenv("KTK_BRANCH_ID")),
		KTKDeviceName:      getenv("KTK_DEVICE_NAME", "ktk-schedule"),
		KTKDebugSchedule:   debugSchedule,
		KTKCallPresetPath:  strings.TrimSpace(os.Getenv("KTK_CALL_PRESET_PATH")),
		DefaultGroup:       defaultGroup,
		DefaultSubgroup:    getenv("DEFAULT_SUBGROUP", "1"),
		OwnerTelegramID:    ownerTelegramID,
		NotifyTime:         getenv("NOTIFY_TIME", "07:30"),
		Timezone:           getenv("TIMEZONE", "Asia/Yekaterinburg"),
	}

	if cfg.BotToken == "" {
		return Config{}, errors.New("BOT_TOKEN is empty")
	}
	if err := validateBotToken(cfg.BotToken); err != nil {
		return Config{}, err
	}
	if len(cfg.CredentialsSecret) < 32 {
		return Config{}, errors.New("CREDENTIALS_SECRET must be at least 32 characters")
	}

	return cfg, nil
}

func getenv(key, fallback string) string {
	value := os.Getenv(key)
	if value == "" {
		return fallback
	}
	return value
}

func getenvBool(key string, fallback bool) (bool, error) {
	value := strings.TrimSpace(os.Getenv(key))
	if value == "" {
		return fallback, nil
	}

	parsed, err := strconv.ParseBool(value)
	if err != nil {
		return false, err
	}
	return parsed, nil
}

func getenvInt64(key string, fallback int64) (int64, error) {
	value := strings.TrimSpace(os.Getenv(key))
	if value == "" {
		return fallback, nil
	}

	return strconv.ParseInt(value, 10, 64)
}

func validateBotToken(token string) error {
	botID, secret, ok := strings.Cut(token, ":")
	if !ok || botID == "" || secret == "" {
		return errors.New("BOT_TOKEN has invalid format; expected token from @BotFather like 123456:ABC")
	}

	for _, r := range botID {
		if r < '0' || r > '9' {
			return errors.New("BOT_TOKEN has invalid bot id; copy the full token from @BotFather")
		}
	}

	return nil
}

func parseLogLevel(value string) slog.Level {
	switch strings.ToLower(strings.TrimSpace(value)) {
	case "debug":
		return slog.LevelDebug
	case "info":
		return slog.LevelInfo
	case "warn", "warning":
		return slog.LevelWarn
	case "error":
		return slog.LevelError
	default:
		return slog.LevelInfo
	}
}
