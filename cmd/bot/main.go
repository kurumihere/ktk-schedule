package main

import (
	"context"
	"os/signal"
	"syscall"

	"ktk-schedule/internal/app"
	"ktk-schedule/internal/config"
	"ktk-schedule/internal/logger"
)

func main() {
	ctx, stop := signal.NotifyContext(context.Background(), syscall.SIGINT, syscall.SIGTERM)
	defer stop()

	cfg, err := config.Load()
	if err != nil {
		logger.Error("Config load: %v", err)
		stop()
		return
	}

	logger.SetLevel(cfg.LogLevel)

	application, err := app.New(cfg)
	if err != nil {
		logger.Error("App init: %v", err)
		stop()
		return
	}

	if err := application.Run(ctx); err != nil {
		logger.Error("App run: %v", err)
	}

	application.Close()
	logger.Info("Bot stopped gracefully")
}
