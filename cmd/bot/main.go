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
		os.Exit(1)
	}

	slog.SetDefault(slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{
		Level: cfg.LogLevel,
	})))

	application, err := app.New(cfg)
	if err != nil {
		slog.Error("app init", "error", err)
		os.Exit(1)
	}

	if err := application.Run(ctx); err != nil {
		slog.Error("app run", "error", err)
		os.Exit(1)
	}

	slog.Info("bot stopped gracefully")
}
