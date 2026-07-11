package app

import (
	"ktk-schedule/internal/logger"
	"sync"
	"time"
)

type circuitBreaker struct {
	mu           sync.Mutex
	failures     int
	state        breakerState
	lastFailure  time.Time
	threshold    int
	resetTimeout time.Duration
}

type breakerState string

const (
	stateClosed   breakerState = "closed"
	stateOpen     breakerState = "open"
	stateHalfOpen breakerState = "halfopen"
)

func newCircuitBreaker(threshold int, resetTimeout time.Duration) *circuitBreaker {
	return &circuitBreaker{
		state:        stateClosed,
		threshold:    threshold,
		resetTimeout: resetTimeout,
	}
}

func (cb *circuitBreaker) Allow() bool {
	cb.mu.Lock()
	defer cb.mu.Unlock()

	switch cb.state {
	case stateClosed:
		return true
	case stateOpen:
		if time.Since(cb.lastFailure) > cb.resetTimeout {
			cb.state = stateHalfOpen
			logger.Info("Circuit breaker transitioning to half-open")
			return true
		}
		return false
	case stateHalfOpen:
		return true
	}
	return false
}

func (cb *circuitBreaker) RecordSuccess() {
	cb.mu.Lock()
	defer cb.mu.Unlock()

	if cb.state == stateHalfOpen {
		logger.Info("Circuit breaker closed after half-open success")
	}
	cb.failures = 0
	cb.state = stateClosed
}

func (cb *circuitBreaker) RecordFailure() {
	cb.mu.Lock()
	defer cb.mu.Unlock()

	cb.failures++
	cb.lastFailure = time.Now()

	if cb.failures >= cb.threshold && cb.state != stateOpen {
		cb.state = stateOpen
		logger.Warn("Circuit breaker opened after %d failures", cb.failures)
	}
}

func (cb *circuitBreaker) State() breakerState {
	cb.mu.Lock()
	defer cb.mu.Unlock()
	return cb.state
}
