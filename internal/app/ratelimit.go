package app

import (
	"sync"
	"time"
)

const (
	scheduleCooldown = 15 * time.Second
	loginCooldown    = 30 * time.Second
	sessionTTL       = 10 * time.Minute
	cleanupInterval  = 5 * time.Minute
)

type rateLimiter struct {
	mu       sync.Mutex
	values   map[int64]time.Time
	cooldown time.Duration
	stopCh   chan struct{}
}

func newRateLimiter() *rateLimiter {
	r := &rateLimiter{
		values:   make(map[int64]time.Time),
		cooldown: scheduleCooldown,
		stopCh:   make(chan struct{}),
	}
	go r.cleanupLoop()
	return r
}

func (r *rateLimiter) allow(key int64) bool {
	r.mu.Lock()
	defer r.mu.Unlock()

	last, ok := r.values[key]
	now := time.Now()

	if ok && now.Sub(last) < r.cooldown {
		return false
	}

	r.values[key] = now
	return true
}

func newLoginRateLimiter() *rateLimiter {
	r := &rateLimiter{
		values:   make(map[int64]time.Time),
		cooldown: loginCooldown,
		stopCh:   make(chan struct{}),
	}
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
	r.mu.Lock()
	defer r.mu.Unlock()

	now := time.Now()
	for key, last := range r.values {
		if now.Sub(last) > sessionTTL {
			delete(r.values, key)
		}
	}
}
