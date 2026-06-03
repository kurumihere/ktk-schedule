package app

import (
	"sync"
	"time"
)

const (
	scheduleRateWindow    = 10 * time.Second
	scheduleRateLimit     = 8
	scheduleBlockDuration = 20 * time.Second
	loginRateWindow       = time.Minute
	loginRateLimit        = 5
	loginBlockDuration    = 5 * time.Minute
	cleanupInterval       = 5 * time.Minute
)

type rateLimitEntry struct {
	events       []time.Time
	blockedUntil time.Time
}

type rateLimiter struct {
	mu       sync.Mutex
	values   map[int64]rateLimitEntry
	window   time.Duration
	limit    int
	blockFor time.Duration
	stopCh   chan struct{}
}

func newRateLimiter() *rateLimiter {
	r := newRateLimiterWithConfig(scheduleRateWindow, scheduleRateLimit, scheduleBlockDuration)
	go r.cleanupLoop()
	return r
}

func newRateLimiterWithConfig(window time.Duration, limit int, blockFor time.Duration) *rateLimiter {
	return &rateLimiter{
		values:   make(map[int64]rateLimitEntry),
		window:   window,
		limit:    limit,
		blockFor: blockFor,
		stopCh:   make(chan struct{}),
	}
}

func (r *rateLimiter) allow(key int64) bool {
	return r.allowAt(key, time.Now())
}

func (r *rateLimiter) allowAt(key int64, now time.Time) bool {
	r.mu.Lock()
	defer r.mu.Unlock()

	if r.limit <= 0 {
		return false
	}

	entry := r.values[key]
	if now.Before(entry.blockedUntil) {
		return false
	}

	cutoff := now.Add(-r.window)
	events := entry.events[:0]
	for _, event := range entry.events {
		if event.After(cutoff) {
			events = append(events, event)
		}
	}
	entry.events = events

	if len(entry.events) >= r.limit {
		entry.blockedUntil = now.Add(r.blockFor)
		r.values[key] = entry
		return false
	}

	entry.events = append(entry.events, now)
	r.values[key] = entry
	return true
}

func newLoginRateLimiter() *rateLimiter {
	r := newRateLimiterWithConfig(loginRateWindow, loginRateLimit, loginBlockDuration)
	go r.cleanupLoop()
	return r
}

func (r *rateLimiter) Close() {
	close(r.stopCh)
}

func (r *rateLimiter) cleanupLoop() {
	ticker := time.NewTicker(cleanupInterval)
	defer ticker.Stop()

	for {
		select {
		case <-r.stopCh:
			return
		case <-ticker.C:
			r.cleanup()
		}
	}
}

func (r *rateLimiter) cleanup() {
	r.cleanupAt(time.Now())
}

func (r *rateLimiter) cleanupAt(now time.Time) {
	r.mu.Lock()
	defer r.mu.Unlock()

	for key, entry := range r.values {
		cutoff := now.Add(-r.window)
		events := entry.events[:0]
		for _, event := range entry.events {
			if event.After(cutoff) {
				events = append(events, event)
			}
		}
		entry.events = events
		if len(entry.events) == 0 && !now.Before(entry.blockedUntil) {
			delete(r.values, key)
			continue
		}
		r.values[key] = entry
	}
}
