package config

import (
	"errors"
	"fmt"
	"net"
	"os"
	"strconv"
	"strings"

	"github.com/joho/godotenv"
	"ktk-schedule/internal/logger"
)

type Config struct {
	BotToken          string
	BaseURL           string
	DatabasePath      string
	CredentialsSecret string
	KTKDeviceName     string
	KTKDebugSchedule  bool
	DefaultGroup      int
	DefaultSubgroup   string
	OwnerTelegramID   int64
	NotifyTime        string
	Timezone          string
	LogLevel          logger.Level
	HealthPort        string
	HealthAddr        string
	PprofEnabled      bool
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
	pprofEnabled, err := getenvBool("PPROF_ENABLED", false)
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

	logLevel := logger.ParseLevel(os.Getenv("LOG_LEVEL"))
	healthPort := getenv("HEALTH_PORT", "8080")
	healthAddr, err := normalizeHealthAddr(os.Getenv("HEALTH_ADDR"), healthPort)
	if err != nil {
		return Config{}, err
	}

	cfg := Config{
		BotToken:          strings.TrimSpace(os.Getenv("BOT_TOKEN")),
		BaseURL:           getenv("KTK_BASE_URL", "https://workspace.ktk-45.ru"),
		DatabasePath:      getenv("DATABASE_PATH", "ktk-schedule.db"),
		CredentialsSecret: strings.TrimSpace(os.Getenv("CREDENTIALS_SECRET")),
		LogLevel:          logLevel,
		HealthPort:        healthPort,
		HealthAddr:        healthAddr,
		KTKDeviceName:     getenv("KTK_DEVICE_NAME", "ktk-schedule"),
		KTKDebugSchedule:  debugSchedule,
		DefaultGroup:      defaultGroup,
		DefaultSubgroup:   getenv("DEFAULT_SUBGROUP", "1"),
		OwnerTelegramID:   ownerTelegramID,
		NotifyTime:        getenv("NOTIFY_TIME", "07:30"),
		Timezone:          getenv("TIMEZONE", "Asia/Yekaterinburg"),
		PprofEnabled:      pprofEnabled,
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

func normalizeHealthAddr(addr, port string) (string, error) {
	addr = strings.TrimSpace(addr)
	if addr != "" {
		if _, _, err := net.SplitHostPort(addr); err != nil {
			return "", fmt.Errorf("HEALTH_ADDR must be host:port: %w", err)
		}
		return addr, nil
	}

	port = strings.TrimSpace(port)
	if port == "" {
		port = "8080"
	}
	if _, err := strconv.Atoi(port); err != nil {
		return "", fmt.Errorf("HEALTH_PORT must be a port number: %w", err)
	}
	return net.JoinHostPort("127.0.0.1", port), nil
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
