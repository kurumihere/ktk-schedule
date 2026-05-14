package app

import (
	"sync"
	"time"
)

const scheduleCooldown = 30 * time.Second

type rateLimiter struct {
	mu     sync.Mutex
	values map[int64]time.Time
}

func newRateLimiter() *rateLimiter {
	return &rateLimiter{values: make(map[int64]time.Time)}
}

func (r *rateLimiter) allow(key int64) bool {
	r.mu.Lock()
	defer r.mu.Unlock()

	last, ok := r.values[key]
	now := time.Now()
	r.values[key] = now

	if !ok {
		return true
	}

	return now.Sub(last) >= scheduleCooldown
}
