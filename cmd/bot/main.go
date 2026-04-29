package main

import (
	"context"
	"log"
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
		log.Fatal(err)
	}

	application, err := app.New(cfg)
	if err != nil {
		log.Fatal(err)
	}
	defer application.Close()

	if err := application.Run(ctx); err != nil {
		log.Fatal(err)
	}
}
