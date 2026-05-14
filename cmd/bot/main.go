package main

import (
	"context"
	"log/slog"
	"os"
	"os/signal"
	"syscall"

	"ktk-schedule/internal/app"
	"ktk-schedule/internal/config"
)

func main() {
	ctx, stop := signal.NotifyContext(context.Background(), syscall.SIGINT, syscall.SIGTERM)
	defer stop()

	cfg, err := config.Load()
	if err != nil {
		slog.Error("config load", "error", err)
		stop()
		return
	}

	slog.SetDefault(slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{
		Level: cfg.LogLevel,
	})))

	application, err := app.New(cfg)
	if err != nil {
		slog.Error("app init", "error", err)
		stop()
		return
	}

	if err := application.Run(ctx); err != nil {
		slog.Error("app run", "error", err)
		stop()
		return
	}

	slog.Info("bot stopped gracefully")
}
