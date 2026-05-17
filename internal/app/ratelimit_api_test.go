package app

import (
	"context"
	"errors"
	"testing"
	"time"

	telegram "github.com/go-telegram/bot"
)

func TestIsRateLimited(t *testing.T) {
	retryAfter := &telegram.TooManyRequestsError{
		Message:    "too many requests",
		RetryAfter: 5,
	}
	if !isRateLimited(retryAfter) {
		t.Error("expected isRateLimited to return true for TooManyRequestsError")
	}
	if isRateLimited(errors.New("some error")) {
		t.Error("expected isRateLimited to return false for regular error")
	}
}

func TestHandleRateLimitRespectsRetryAfter(t *testing.T) {
	retryAfter := &telegram.TooManyRequestsError{
		Message:    "too many requests",
		RetryAfter: 1,
	}

	start := time.Now()
	ctx := context.Background()

	got := handleRateLimit(ctx, retryAfter)
	elapsed := time.Since(start)

	if !got {
		t.Error("expected handleRateLimit to return true after waiting")
	}
	if elapsed < 900*time.Millisecond {
		t.Errorf("expected to wait ~1s, got %v", elapsed)
	}
}

func TestHandleRateLimitCancellable(t *testing.T) {
	retryAfter := &telegram.TooManyRequestsError{
		Message:    "too many requests",
		RetryAfter: 60,
	}

	ctx, cancel := context.WithCancel(context.Background())
	go func() {
		time.Sleep(100 * time.Millisecond)
		cancel()
	}()

	start := time.Now()
	got := handleRateLimit(ctx, retryAfter)
	elapsed := time.Since(start)

	if got {
		t.Error("expected handleRateLimit to return false when context cancelled")
	}
	if elapsed > 1*time.Second {
		t.Errorf("expected fast cancellation, got %v", elapsed)
	}
}

func TestHandleRateLimitNonRateLimitError(t *testing.T) {
	ctx := context.Background()
	got := handleRateLimit(ctx, errors.New("regular error"))
	if !got {
		t.Error("expected handleRateLimit to return true for non-rate-limit error")
	}
}
