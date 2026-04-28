package config

import (
	"errors"
	"os"
	"strconv"

	"github.com/joho/godotenv"
)

type Config struct {
	BotToken     string
	BaseURL      string
	DatabasePath string
	DefaultGroup int
	NotifyTime   string
	Timezone     string
}

func Load() (Config, error) {
	_ = godotenv.Load()

	defaultGroup, err := strconv.Atoi(getenv("DEFAULT_GROUP_ID", "269"))
	if err != nil {
		return Config{}, err
	}

	cfg := Config{
		BotToken:     os.Getenv("BOT_TOKEN"),
		BaseURL:      getenv("KTK_BASE_URL", "https://workspace.ktk-45.ru"),
		DatabasePath: getenv("DATABASE_PATH", "ktk-schedule.db"),
		DefaultGroup: defaultGroup,
		NotifyTime:   getenv("NOTIFY_TIME", "07:30"),
		Timezone:     getenv("TIMEZONE", "Europe/Helsinki"),
	}

	if cfg.BotToken == "" {
		return Config{}, errors.New("BOT_TOKEN is empty")
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
