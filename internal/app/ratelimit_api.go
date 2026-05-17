package app

import (
	"context"
	"errors"
	"log/slog"
	"time"

	telegram "github.com/go-telegram/bot"
)

const (
	maxRetries     = 3
	baseRetryDelay = 1 * time.Second
)

func sendMessageWithRetry(ctx context.Context, bot *telegram.Bot, params *telegram.SendMessageParams) error {
	var lastErr error
	for attempt := range maxRetries {
		if attempt > 0 {
			if !handleRateLimit(ctx, lastErr) {
				return lastErr
			}
		}

		_, err := bot.SendMessage(ctx, params)
		if err == nil {
			return nil
		}

		lastErr = err
		if !isRateLimited(err) {
			return err
		}
	}
	return lastErr
}

func copyMessageWithRetry(ctx context.Context, bot *telegram.Bot, params *telegram.CopyMessageParams) error {
	var lastErr error
	for attempt := range maxRetries {
		if attempt > 0 {
			if !handleRateLimit(ctx, lastErr) {
				return lastErr
			}
		}

		_, err := bot.CopyMessage(ctx, params)
		if err == nil {
			return nil
		}

		lastErr = err
		if !isRateLimited(err) {
			return err
		}
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

	slog.Warn("rate limited, waiting", "retry_after", tooMany.RetryAfter)

	select {
	case <-ctx.Done():
		return false
	case <-time.After(wait):
		return true
	}
}
