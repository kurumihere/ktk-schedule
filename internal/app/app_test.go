package app

import (
	"errors"
	"fmt"
	"strings"
	"testing"
	"time"

	telegram "github.com/go-telegram/bot"
	"github.com/go-telegram/bot/models"

	"ktk-schedule/internal/ktk"
)

func TestCommandArgs(t *testing.T) {
	tests := []struct {
		input string
		want  string
	}{
		{"/schedule 01.09", "01.09"},
		{"/schedule", ""},
		{"/login user pass", "user pass"},
		{"/group  269 ", "269"},
		{"", ""},
	}
	for _, tt := range tests {
		got := commandArgs(tt.input)
		if got != tt.want {
			t.Errorf("commandArgs(%q) = %q, want %q", tt.input, got, tt.want)
		}
	}
}

func TestHasScheduleEndpoint(t *testing.T) {
	if hasScheduleEndpoint(ktk.Endpoints{SchedulePath: ""}) {
		t.Error("expected false for empty path")
	}
	if !hasScheduleEndpoint(ktk.Endpoints{SchedulePath: "/schedule"}) {
		t.Error("expected true for non-empty path")
	}
}

func TestIsMessageNotModified(t *testing.T) {
	if isMessageNotModified(errors.New("some other error")) {
		t.Error("expected false for unrelated error")
	}
	wrapped := fmt.Errorf("%w, %s", telegram.ErrorBadRequest, "message is not modified")
	if !isMessageNotModified(wrapped) {
		t.Error("expected true for wrapped message not modified")
	}
	if isMessageNotModified(fmt.Errorf("%w, %s", telegram.ErrorBadRequest, "invalid chat id")) {
		t.Error("expected false for other bad request error")
	}
}

func TestTelegramSenderID(t *testing.T) {
	from := &models.User{ID: 123}
	msg := &models.Message{From: from, Chat: models.Chat{ID: 456}}
	if id := telegramSenderID(msg); id != 123 {
		t.Errorf("expected 123, got %d", id)
	}

	msgNoFrom := &models.Message{Chat: models.Chat{ID: 456}}
	if id := telegramSenderID(msgNoFrom); id != 456 {
		t.Errorf("expected 456, got %d", id)
	}
}

func TestClientSubgroupOrDefault(t *testing.T) {
	if got := clientSubgroupOrDefault(nil, "2"); got != "right" {
		t.Errorf("expected right, got %s", got)
	}

	if got := clientSubgroupOrDefault(nil, "invalid"); got != "left" {
		t.Errorf("expected left fallback, got %s", got)
	}
}

func TestRateLimiter(t *testing.T) {
	rl := newRateLimiter()
	if !rl.allow(1) {
		t.Error("first request must be allowed")
	}
	if !rl.allow(2) {
		t.Error("different key must be allowed")
	}
	if rl.allow(1) {
		t.Error("immediate second request must be denied")
	}
}

func TestSessionCopy(t *testing.T) {
	loc := time.FixedZone("test", 0)
	orig := &Session{
		Subgroup:         "left",
		ShowAllSubgroups: false,
		CurrentIndex:     2,
		Schedule: []ktk.ScheduleDay{
			{Date: "2026-05-14T00:00:00Z", Subjects: []ktk.ScheduleItem{
				{Discipline: "Math", Pair: 1},
				{Discipline: "PE", Pair: 2},
			}},
		},
		Halls: ktk.LectureHallMap{
			1: {ID: 1, Number: "202", Housing: 1},
		},
		CallPresets: ktk.CallPresetMap{
			1: {ID: 1, Name: "Default"},
		},
		AbsenceMarks: []ktk.AbsenceMark{
			{Digit: 4, Caption: "sick"},
		},
		WeekStart: ktk.WeekStart(time.Now(), loc),
	}

	cp := orig.copy()
	if cp == orig {
		t.Fatal("copy must be a different pointer")
	}
	if cp.Subgroup != orig.Subgroup {
		t.Fatal("subgroup mismatch")
	}
	if cp.CurrentIndex != orig.CurrentIndex {
		t.Fatal("currentIndex mismatch")
	}
	if !cp.WeekStart.Equal(orig.WeekStart) {
		t.Fatal("weekStart mismatch")
	}

	cp.Subgroup = "right"
	cp.CurrentIndex = 0
	if orig.Subgroup == cp.Subgroup {
		t.Fatal("modifying copy must not affect original scalar fields")
	}
}

func TestTelegramErrorWrapping(t *testing.T) {
	err := fmt.Errorf("%w, %s", telegram.ErrorBadRequest, "message is not modified")
	if !errors.Is(err, telegram.ErrorBadRequest) {
		t.Error("message not modified must wrap ErrorBadRequest")
	}
}

func TestSessionCopyNil(t *testing.T) {
	var s *Session
	if s.copy() != nil {
		t.Fatal("nil session must return nil")
	}
}

func TestCommandArgsSpaces(t *testing.T) {
	got := commandArgs("/schedule    01.09")
	want := "01.09"
	if got != want {
		t.Errorf("expected %q, got %q", want, got)
	}

	if strings.TrimSpace(commandArgs("/schedule  ")) != "" {
		t.Error("expected empty result for trailing spaces")
	}
}

func TestCircuitBreaker(t *testing.T) {
	cb := newCircuitBreaker(3, 100*time.Millisecond)

	for i := 0; i < 3; i++ {
		if !cb.Allow() {
			t.Errorf("expected allowed on iteration %d", i)
		}
		cb.RecordFailure()
	}

	if cb.Allow() {
		t.Error("expected circuit breaker to be open after threshold failures")
	}

	cb.RecordSuccess()
	if !cb.Allow() {
		t.Error("expected circuit breaker to be closed after success")
	}
}

func TestCircuitBreakerHalfOpen(t *testing.T) {
	cb := newCircuitBreaker(2, 50*time.Millisecond)

	cb.RecordFailure()
	cb.RecordFailure()

	if cb.Allow() {
		t.Error("expected circuit breaker to be open")
	}

	time.Sleep(60 * time.Millisecond)

	if !cb.Allow() {
		t.Error("expected circuit breaker to transition to half-open after timeout")
	}

	if cb.State() != stateHalfOpen {
		t.Errorf("expected state half-open, got %s", cb.State())
	}

	cb.RecordSuccess()
	if cb.State() != stateClosed {
		t.Errorf("expected state closed after half-open success, got %s", cb.State())
	}
}
