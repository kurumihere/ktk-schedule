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

	"ktk-schedule/internal/config"
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

func TestIsPrivateUserMessage(t *testing.T) {
	tests := []struct {
		name    string
		message *models.Message
		want    bool
	}{
		{
			name: "private chat owner",
			message: &models.Message{
				From: &models.User{ID: 123},
				Chat: models.Chat{ID: 123, Type: models.ChatTypePrivate},
			},
			want: true,
		},
		{
			name: "group chat",
			message: &models.Message{
				From: &models.User{ID: 123},
				Chat: models.Chat{ID: -456, Type: models.ChatTypeGroup},
			},
		},
		{
			name: "different sender",
			message: &models.Message{
				From: &models.User{ID: 123},
				Chat: models.Chat{ID: 456, Type: models.ChatTypePrivate},
			},
		},
		{name: "missing message"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := isPrivateUserMessage(tt.message); got != tt.want {
				t.Fatalf("isPrivateUserMessage() = %t, want %t", got, tt.want)
			}
		})
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

func TestOwnScheduleSubgroupSettingsPreferLinkedSubgroup(t *testing.T) {
	user := &storage.User{Subgroup: "left", ShowAllSubgroups: true}

	subgroup, showAll := ownScheduleSubgroupSettingsFromValues(user, "2")

	if subgroup != "right" {
		t.Fatalf("expected linked subgroup right, got %q", subgroup)
	}
	if showAll {
		t.Fatal("own group must reset all-subgroups mode")
	}
}

func TestOwnScheduleSubgroupSettingsKeepStoredFallback(t *testing.T) {
	user := &storage.User{Subgroup: "left", PersonalSubgroup: "right", ShowAllSubgroups: true}

	subgroup, showAll := ownScheduleSubgroupSettings(user, &Session{})

	if subgroup != "right" {
		t.Fatalf("expected stored personal subgroup right, got %q", subgroup)
	}
	if showAll {
		t.Fatal("own group must reset all-subgroups mode")
	}
}

func TestOwnScheduleSubgroupSettingsFallbackToCurrentSubgroup(t *testing.T) {
	user := &storage.User{Subgroup: "right", ShowAllSubgroups: true}

	subgroup, showAll := ownScheduleSubgroupSettings(user, &Session{})

	if subgroup != "right" {
		t.Fatalf("expected current subgroup fallback right, got %q", subgroup)
	}
	if showAll {
		t.Fatal("own group must reset all-subgroups mode")
	}
}

func TestOwnScheduleSubgroupSettingsLeavesTeacherSettings(t *testing.T) {
	user := &storage.User{Subgroup: "right", ShowAllSubgroups: true, TeacherHash: "teacher-hash"}

	subgroup, showAll := ownScheduleSubgroupSettings(user, &Session{})

	if subgroup != "right" {
		t.Fatalf("expected teacher subgroup to stay right, got %q", subgroup)
	}
	if !showAll {
		t.Fatal("teacher settings must stay unchanged")
	}
}

func TestSaveUserSubgroupModeKeepsPersonalSubgroup(t *testing.T) {
	cipher, err := credentials.New("app-test-secret-with-32-characters")
	if err != nil {
		t.Fatal(err)
	}
	store, err := storage.New(t.TempDir()+"/test.db", cipher)
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = store.Close() }()

	user := storage.User{
		TelegramID:       101,
		Login:            "student",
		Password:         "password",
		GroupID:          269,
		Subgroup:         "right",
		PersonalSubgroup: "right",
	}
	if err := store.SaveUser(user); err != nil {
		t.Fatal(err)
	}

	app := &App{storage: store}
	if err := app.saveUserSubgroupMode(&user, "left", false, &Session{}); err != nil {
		t.Fatal(err)
	}

	saved, err := store.GetUser(user.TelegramID)
	if err != nil {
		t.Fatal(err)
	}
	if saved.Subgroup != "left" {
		t.Fatalf("expected selected subgroup left, got %q", saved.Subgroup)
	}
	if saved.PersonalSubgroup != "right" {
		t.Fatalf("expected personal subgroup to stay right, got %q", saved.PersonalSubgroup)
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
	rl := newRateLimiterWithConfig(time.Second, 3, 5*time.Second)
	now := time.Date(2026, 6, 3, 12, 0, 0, 0, time.UTC)

	if !rl.allowAt(1, now) {
		t.Error("first request must be allowed")
	}
	if !rl.allowAt(2, now) {
		t.Error("different key must be allowed")
	}
	if !rl.allowAt(1, now.Add(100*time.Millisecond)) || !rl.allowAt(1, now.Add(200*time.Millisecond)) {
		t.Fatal("normal burst must be allowed")
	}
	if rl.allowAt(1, now.Add(300*time.Millisecond)) {
		t.Fatal("excessive burst must be denied")
	}
	if rl.allowAt(1, now.Add(time.Second)) {
		t.Fatal("blocked key must stay denied")
	}
	if !rl.allowAt(1, now.Add(6*time.Second)) {
		t.Fatal("key must be allowed after block expires")
	}
}

func TestSessionCopy(t *testing.T) {
	loc := time.FixedZone("test", 0)
	orig := &Session{
		Subgroup:         "left",
		ShowAllSubgroups: false,
		CurrentIndex:     2,
		AllSchedule: []ktk.ScheduleDay{
			{Date: "2026-05-14T00:00:00Z", Subjects: []ktk.ScheduleItem{
				{Discipline: "Raw Math", Pair: 1},
			}},
		},
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
		documentCache: map[int]ktk.DocumentMetadata{
			10: {ID: 10, Caption: "task.pdf", Icon: "pdf"},
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
	cp.AllSchedule[0].Subjects[0].Discipline = "Changed"
	cp.documentCache[10] = ktk.DocumentMetadata{ID: 10, Caption: "changed.pdf", Icon: "pdf"}
	if orig.Subgroup == cp.Subgroup {
		t.Fatal("modifying copy must not affect original scalar fields")
	}
	if orig.AllSchedule[0].Subjects[0].Discipline == cp.AllSchedule[0].Subjects[0].Discipline {
		t.Fatal("modifying copy must not affect original all schedule")
	}
	if orig.documentCache[10].Caption == cp.documentCache[10].Caption {
		t.Fatal("modifying copy must not affect original document cache")
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

func TestStartupNotificationDate(t *testing.T) {
	loc := time.FixedZone("test", 5*60*60)
	tests := []struct {
		name      string
		now       time.Time
		wantDate  string
		wantRun   bool
		wantError bool
	}{
		{
			name:     "inside startup window",
			now:      time.Date(2026, 6, 1, 7, 31, 0, 0, loc),
			wantDate: "2026-06-01",
			wantRun:  true,
		},
		{
			name:    "before notify time",
			now:     time.Date(2026, 6, 1, 7, 29, 59, 0, loc),
			wantRun: false,
		},
		{
			name:    "after startup window",
			now:     time.Date(2026, 6, 1, 7, 32, 1, 0, loc),
			wantRun: false,
		},
		{
			name:      "invalid notify time",
			now:       time.Date(2026, 6, 1, 7, 31, 0, 0, loc),
			wantError: true,
		},
	}

	for _, tt := range tests {
		notifyTime := "07:30"
		if tt.wantError {
			notifyTime = "invalid"
		}

		gotDate, gotRun, err := startupNotificationDate(tt.now, notifyTime, loc)
		if (err != nil) != tt.wantError {
			t.Fatalf("%s: error = %v, want error %v", tt.name, err, tt.wantError)
		}
		if gotRun != tt.wantRun {
			t.Fatalf("%s: run = %v, want %v", tt.name, gotRun, tt.wantRun)
		}
		if gotDate != tt.wantDate {
			t.Fatalf("%s: date = %q, want %q", tt.name, gotDate, tt.wantDate)
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

	app.deleteSession(1)
	if app.getSession(1) != nil {
		t.Fatal("deleteSession must remove existing session")
	}
}

func TestRefilterSessionScheduleAppliesSubgroupChange(t *testing.T) {
	loc := time.FixedZone("test", 0)
	weekStart := time.Date(2026, 6, 1, 6, 0, 0, 0, loc)
	session := &Session{
		AllSchedule: []ktk.ScheduleDay{{
			Date: "2026-06-01",
			Subjects: []ktk.ScheduleItem{
				{Discipline: "Class hour", Subgroup: "middle"},
				{Discipline: "Programming", Subgroup: "1-я подгруппа"},
				{Discipline: "Networks", Subgroup: "2-я подгруппа"},
			},
		}},
		Schedule: []ktk.ScheduleDay{{
			Date: "2026-06-01",
			Subjects: []ktk.ScheduleItem{
				{Discipline: "Class hour", Subgroup: "middle"},
				{Discipline: "Programming", Subgroup: "1-я подгруппа"},
			},
		}},
		CurrentIndex: 0,
		WeekStart:    weekStart,
		Subgroup:     "right",
	}

	refilterSessionSchedule(session, loc)

	if len(session.Schedule) != 1 {
		t.Fatalf("unexpected day count: %d", len(session.Schedule))
	}
	got := session.Schedule[0].Subjects
	if len(got) != 2 {
		t.Fatalf("unexpected subjects: %#v", got)
	}
	if got[0].Discipline != "Class hour" || got[1].Discipline != "Networks" {
		t.Fatalf("unexpected filtered subjects: %#v", got)
	}
}

func TestShouldUseGroupSchedule(t *testing.T) {
	app := &App{cfg: config.Config{DefaultGroup: 269, DefaultSubgroup: "1"}}

	tests := []struct {
		name     string
		groupID  int
		subgroup string
		showAll  bool
		want     bool
	}{
		{name: "default personal subgroup", groupID: 269, subgroup: "left", want: false},
		{name: "other group", groupID: 268, subgroup: "left", want: true},
		{name: "other subgroup", groupID: 269, subgroup: "right", want: true},
		{name: "show all subgroups", groupID: 269, subgroup: "left", showAll: true, want: true},
	}

	for _, tt := range tests {
		got := app.shouldUseGroupScheduleValues(tt.groupID, tt.subgroup, tt.showAll)
		if got != tt.want {
			t.Fatalf("%s: got %v, want %v", tt.name, got, tt.want)
		}
	}
}

func TestScheduleCacheSeparatesPersonalAndGroupSchedules(t *testing.T) {
	cache := newScheduleCache()
	personal := []ktk.ScheduleDay{{Date: "2026-06-01", Subjects: []ktk.ScheduleItem{{Discipline: "Personal"}}}}
	group := []ktk.ScheduleDay{{Date: "2026-06-01", Subjects: []ktk.ScheduleItem{{Discipline: "Group"}}}}

	cache.setWithMode(269, "2026-06-01", "", false, personal)
	cache.setWithMode(269, "2026-06-01", "", true, group)

	gotPersonal, ok := cache.getWithMode(269, "2026-06-01", "", false)
	if !ok || gotPersonal[0].Subjects[0].Discipline != "Personal" {
		t.Fatalf("unexpected personal cache value: %#v", gotPersonal)
	}

	gotGroup, ok := cache.getWithMode(269, "2026-06-01", "", true)
	if !ok || gotGroup[0].Subjects[0].Discipline != "Group" {
		t.Fatalf("unexpected group cache value: %#v", gotGroup)
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
		ptr.Store(&Session{lastAccessUnix: time.Now().Add(-48 * time.Hour).Unix()})
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
	defer func() { _ = store.Close() }()

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

	days, err := app.loadSchedule(context.Background(), nil, 269, "", time.Date(2026, 6, 1, 12, 0, 0, 0, time.UTC), false)
	if err != nil {
		t.Fatal(err)
	}
	if len(days) != 1 || len(days[0].Subjects) != 1 || days[0].Subjects[0].Discipline != "Math" {
		t.Fatalf("unexpected cached schedule: %#v", days)
	}
}
