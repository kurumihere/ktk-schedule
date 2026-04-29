package config

import (
	"errors"
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
	DefaultGroup       int
	NotifyTime         string
	Timezone           string
}

func Load() (Config, error) {
	_ = godotenv.Load()

	defaultGroup, err := strconv.Atoi(getenv("DEFAULT_GROUP_ID", "269"))
	if err != nil {
		return Config{}, err
	}

	cfg := Config{
		BotToken:           os.Getenv("BOT_TOKEN"),
		BaseURL:            getenv("KTK_BASE_URL", "https://workspace.ktk-45.ru"),
		DatabasePath:       getenv("DATABASE_PATH", "ktk-schedule.db"),
		CredentialsSecret:  strings.TrimSpace(os.Getenv("CREDENTIALS_SECRET")),
		KTKSignInPath:      getenv("KTK_SIGN_IN_PATH", "/sign-in"),
		KTKSchedulePath:    strings.TrimSpace(os.Getenv("KTK_SCHEDULE_PATH")),
		KTKLectureHallPath: strings.TrimSpace(os.Getenv("KTK_LECTURE_HALL_PATH")),
		KTKBranchID:        strings.TrimSpace(os.Getenv("KTK_BRANCH_ID")),
		KTKDeviceName:      getenv("KTK_DEVICE_NAME", "ktk-schedule"),
		DefaultGroup:       defaultGroup,
		NotifyTime:         getenv("NOTIFY_TIME", "07:30"),
		Timezone:           getenv("TIMEZONE", "Asia/Yekaterinburg"),
	}

	if cfg.BotToken == "" {
		return Config{}, errors.New("BOT_TOKEN is empty")
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
