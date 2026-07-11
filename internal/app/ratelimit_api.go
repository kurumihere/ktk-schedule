package app

import (
	"context"
	"errors"
	"ktk-schedule/internal/logger"
	"time"

	telegram "github.com/go-telegram/bot"
)

const (
	maxRetries     = 3
	baseRetryDelay = 1 * time.Second
)

func sendMessageWithRetry(ctx context.Context, bot *telegram.Bot, params *telegram.SendMessageParams) error {
	return withRetry(ctx, func() error {
		_, err := bot.SendMessage(ctx, params)
		return err
	})
}

func copyMessageWithRetry(ctx context.Context, bot *telegram.Bot, params *telegram.CopyMessageParams) error {
	return withRetry(ctx, func() error {
		_, err := bot.CopyMessage(ctx, params)
		return err
	})
}

func withRetry(ctx context.Context, fn func() error) error {
	var lastErr error
	for attempt := range maxRetries {
		if attempt > 0 {
			if !handleRateLimit(ctx, lastErr) {
				return lastErr
			}
		}

		err := fn()
		if err == nil {
			return nil
		} else if !isRateLimited(err) {
			return err
		}
		lastErr = err
	}
	return lastErr
}

func isRateLimited(err error) bool {
	var tooMany *telegram.TooManyRequestsError
	return errors.As(err, &tooMany)
}

func handleRateLimit(ctx context.Context, err error) bool {
	var tooMany *telegram.TooManyRequestsError
	if !errors.As(err, &tooMany) {
		return true
	}

	wait := time.Duration(tooMany.RetryAfter) * time.Second
	if wait < baseRetryDelay {
		wait = baseRetryDelay
	}

	logger.Warn("Telegram rate limit reached; waiting %d seconds", tooMany.RetryAfter)

	select {
	case <-ctx.Done():
		return false
	case <-time.After(wait):
		return true
	}
}
