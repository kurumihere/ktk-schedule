package app

import (
	"sync"
	"testing"
	"time"
)

func TestCircuitBreakerInitialState(t *testing.T) {
	cb := newCircuitBreaker(3, 100*time.Millisecond)

	if cb.State() != stateClosed {
		t.Fatalf("expected closed, got %s", cb.State())
	}
	if !cb.Allow() {
		t.Fatal("expected Allow() to return true in closed state")
	}
}

func TestCircuitBreakerOpensAfterThreshold(t *testing.T) {
	cb := newCircuitBreaker(3, 100*time.Millisecond)

	cb.RecordFailure()
	cb.RecordFailure()
	if cb.State() != stateClosed {
		t.Fatal("expected closed after 2 failures (below threshold)")
	}

	cb.RecordFailure()
	if cb.State() != stateOpen {
		t.Fatalf("expected open after 3 failures, got %s", cb.State())
	}
}

func TestCircuitBreakerBlocksWhenOpen(t *testing.T) {
	cb := newCircuitBreaker(2, time.Second)

	cb.RecordFailure()
	cb.RecordFailure()

	if cb.Allow() {
		t.Fatal("expected Allow() to return false when open")
	}
}

func TestCircuitBreakerTransitionsToHalfOpen(t *testing.T) {
	cb := newCircuitBreaker(2, 50*time.Millisecond)

	cb.RecordFailure()
	cb.RecordFailure()

	time.Sleep(60 * time.Millisecond)

	if !cb.Allow() {
		t.Fatal("expected Allow() to return true after reset timeout")
	}
	if cb.State() != stateHalfOpen {
		t.Fatalf("expected halfopen, got %s", cb.State())
	}
}

func TestCircuitBreakerClosesOnHalfOpenSuccess(t *testing.T) {
	cb := newCircuitBreaker(2, 50*time.Millisecond)

	cb.RecordFailure()
	cb.RecordFailure()
	time.Sleep(60 * time.Millisecond)
	cb.Allow() // transitions to half-open

	cb.RecordSuccess()

	if cb.State() != stateClosed {
		t.Fatalf("expected closed after half-open success, got %s", cb.State())
	}
}

func TestCircuitBreakerReopensOnHalfOpenFailure(t *testing.T) {
	cb := newCircuitBreaker(2, 50*time.Millisecond)

	cb.RecordFailure()
	cb.RecordFailure()
	time.Sleep(60 * time.Millisecond)
	cb.Allow() // transitions to half-open

	cb.RecordFailure()

	if cb.State() != stateOpen {
		t.Fatalf("expected open after half-open failure, got %s", cb.State())
	}
}

func TestCircuitBreakerSuccessResetsFailures(t *testing.T) {
	cb := newCircuitBreaker(3, 100*time.Millisecond)

	cb.RecordFailure()
	cb.RecordFailure()
	cb.RecordSuccess()

	cb.RecordFailure()
	if cb.State() != stateClosed {
		t.Fatalf("expected closed after success reset, got %s", cb.State())
	}
}

func TestCircuitBreakerRepeatedOpenClose(t *testing.T) {
	cb := newCircuitBreaker(2, 50*time.Millisecond)

	for i := 0; i < 3; i++ {
		cb.RecordFailure()
		cb.RecordFailure()
		if cb.State() != stateOpen {
			t.Fatalf("iteration %d: expected open, got %s", i, cb.State())
		}

		time.Sleep(60 * time.Millisecond)
		cb.Allow()
		if cb.State() != stateHalfOpen {
			t.Fatalf("iteration %d: expected halfopen, got %s", i, cb.State())
		}

		cb.RecordSuccess()
		if cb.State() != stateClosed {
			t.Fatalf("iteration %d: expected closed, got %s", i, cb.State())
		}
	}
}

func TestCircuitBreakerConcurrentAccess(t *testing.T) {
	cb := newCircuitBreaker(100, 10*time.Millisecond)

	var wg sync.WaitGroup
	for i := 0; i < 50; i++ {
		wg.Add(3)
		go func() {
			defer wg.Done()
			cb.Allow()
		}()
		go func() {
			defer wg.Done()
			cb.RecordFailure()
		}()
		go func() {
			defer wg.Done()
			cb.RecordSuccess()
		}()
	}
	wg.Wait()
}
