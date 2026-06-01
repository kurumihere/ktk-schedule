package app

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	telegram "github.com/go-telegram/bot"
	"github.com/go-telegram/bot/models"

	"ktk-schedule/internal/credentials"
	"ktk-schedule/internal/ktk"
	"ktk-schedule/internal/storage"
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

func TestSyncTeacherHashUpdatesUser(t *testing.T) {
	app := &App{}
	user := &storage.User{TelegramID: 42}

	app.syncTeacherHash(user, "teacher-hash")

	if user.TeacherHash != "teacher-hash" {
		t.Fatalf("unexpected teacher hash: %q", user.TeacherHash)
	}
}

func TestHasScheduledSubjects(t *testing.T) {
	if hasScheduledSubjects(nil) {
		t.Fatal("nil schedule must not have subjects")
	}
	if hasScheduledSubjects([]ktk.ScheduleDay{{Date: "2026-05-25", MaxPair: 8}}) {
		t.Fatal("max-pair-only schedule must not have subjects")
	}
	if !hasScheduledSubjects([]ktk.ScheduleDay{{Date: "2026-05-25", Subjects: []ktk.ScheduleItem{{Discipline: "Math", Pair: 1}}}}) {
		t.Fatal("schedule with subject must be detected")
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

func TestFormatUptime(t *testing.T) {
	tests := []struct {
		d    time.Duration
		want string
	}{
		{30 * time.Minute, "30мин"},
		{90 * time.Minute, "1ч 30мин"},
		{25 * time.Hour, "1д 1ч 0мин"},
		{49 * time.Hour, "2д 1ч 0мин"},
	}
	for _, tt := range tests {
		got := formatUptime(tt.d)
		if got != tt.want {
			t.Errorf("formatUptime(%v) = %q, want %q", tt.d, got, tt.want)
		}
	}
}

func TestExtractTelegramDescription(t *testing.T) {
	tests := []struct {
		input string
		want  string
	}{
		{"Bad Request, message is not modified", "message is not modified"},
		{"Bad Request, invalid chat id", "invalid chat id"},
		{"simple error", "simple error"},
	}
	for _, tt := range tests {
		got := extractTelegramDescription(errors.New(tt.input))
		if got != tt.want {
			t.Errorf("extractTelegramDescription(%q) = %q, want %q", tt.input, got, tt.want)
		}
	}
}

func TestSessionAtomicPointer(t *testing.T) {
	app := &App{}
	app.setSession(1, &Session{Subgroup: "left", CurrentIndex: 0})

	s := app.getSession(1)
	if s == nil || s.Subgroup != "left" {
		t.Fatal("expected session with subgroup left")
	}

	app.modifySession(1, func(s *Session) {
		s.Subgroup = "right"
		s.CurrentIndex = 5
	})

	s = app.getSession(1)
	if s == nil || s.Subgroup != "right" || s.CurrentIndex != 5 {
		t.Fatalf("expected subgroup right and index 5, got %s and %d", s.Subgroup, s.CurrentIndex)
	}

	app.modifySession(999, func(s *Session) {
		s.Subgroup = "should-not-exist"
	})
	if app.getSession(999) != nil {
		t.Fatal("modifySession on non-existent session must not create one")
	}
}

func TestSessionConcurrentAccess(t *testing.T) {
	app := &App{}
	app.setSession(1, &Session{Subgroup: "left", CurrentIndex: 0})

	done := make(chan struct{})
	for i := 0; i < 10; i++ {
		go func(n int) {
			for j := 0; j < 100; j++ {
				app.modifySession(1, func(s *Session) {
					s.CurrentIndex++
				})
				s := app.getSession(1)
				if s == nil {
					t.Error("session disappeared")
					return
				}
			}
			done <- struct{}{}
		}(i)
	}

	for i := 0; i < 10; i++ {
		<-done
	}

	s := app.getSession(1)
	if s == nil || s.CurrentIndex != 1000 {
		t.Errorf("expected index 1000, got %d", s.CurrentIndex)
	}
}

func TestHandleCallbackNextFromNoSelectedDay(t *testing.T) {
	app := &App{}
	session := &Session{
		CurrentIndex: -1,
		Schedule: []ktk.ScheduleDay{
			{Date: "2026-06-01", Subjects: []ktk.ScheduleItem{{Discipline: "Math", Pair: 1}}},
			{Date: "2026-06-02", Subjects: []ktk.ScheduleItem{{Discipline: "Physics", Pair: 1}}},
		},
	}

	app.handleCallbackNext(session)

	if session.CurrentIndex != 0 {
		t.Fatalf("expected first day after next from no selection, got %d", session.CurrentIndex)
	}
}

func TestSessionCleanup(t *testing.T) {
	app := &App{}
	app.setSession(1, &Session{Subgroup: "left"})

	if app.sessionCount() != 1 {
		t.Fatal("expected 1 session")
	}

	app.sessions.Range(func(key, value any) bool {
		ptr := value.(*atomic.Pointer[Session])
		ptr.Store(&Session{lastAccessUnix: time.Now().Add(-1 * time.Hour).Unix()})
		return true
	})

	app.cleanupSessions()
	if app.sessionCount() != 0 {
		t.Fatal("expected 0 sessions after cleanup")
	}
}

func TestLoadScheduleUsesPersistentCacheWithoutClient(t *testing.T) {
	cipher, err := credentials.New("app-test-secret-with-32-characters")
	if err != nil {
		t.Fatal(err)
	}
	store, err := storage.New(t.TempDir()+"/test.db", cipher)
	if err != nil {
		t.Fatal(err)
	}
	defer store.Close()

	app := &App{
		storage:       store,
		location:      time.UTC,
		scheduleCache: newScheduleCache(),
	}
	if err := store.SaveScheduleCache(storage.CachedSchedule{
		GroupID:   269,
		WeekStart: "2026-06-01",
		Data:      []byte(`[{"Date":"2026-06-01","Subjects":[{"Discipline":"Math","Pair":1}]}]`),
	}); err != nil {
		t.Fatal(err)
	}

	days, err := app.loadSchedule(context.Background(), nil, 269, "", time.Date(2026, 6, 1, 12, 0, 0, 0, time.UTC))
	if err != nil {
		t.Fatal(err)
	}
	if len(days) != 1 || len(days[0].Subjects) != 1 || days[0].Subjects[0].Discipline != "Math" {
		t.Fatalf("unexpected cached schedule: %#v", days)
	}
}
