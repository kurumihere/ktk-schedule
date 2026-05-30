package ktk

import (
	"net/url"
	"strings"
	"testing"
	"time"
)

func TestFindDateIndexPrefersLocalDateOverTodayFlag(t *testing.T) {
	loc := time.FixedZone("test", 5*60*60)
	days := []ScheduleDay{
		{Date: "2026-04-27T00:00:00Z", Today: true},
		{Date: "2026-04-28T00:00:00Z"},
		{Date: "2026-04-29T00:00:00Z", Subjects: []ScheduleItem{{Discipline: "Math"}}},
	}

	index := FindDateIndex(days, time.Date(2026, 4, 29, 15, 0, 0, 0, loc), loc)
	if index != 2 {
		t.Fatalf("unexpected day index: %d", index)
	}
}

func TestFilterScheduleDaysKeepsCommonAndSelectedSubgroup(t *testing.T) {
	days := []ScheduleDay{{
		Date: "2026-04-29T00:00:00Z",
		Subjects: []ScheduleItem{
			{Discipline: "Class hour", Subgroup: "middle"},
			{Discipline: "Programming", Subgroup: "left"},
			{Discipline: "Networks", Subgroup: "right"},
		},
	}}

	filtered := FilterScheduleDays(days, "1", false)
	if len(filtered[0].Subjects) != 2 {
		t.Fatalf("unexpected subjects: %#v", filtered[0].Subjects)
	}
	if filtered[0].Subjects[0].Discipline != "Class hour" || filtered[0].Subjects[1].Discipline != "Programming" {
		t.Fatalf("unexpected filtered subjects: %#v", filtered[0].Subjects)
	}
}

func TestFilterScheduleDaysCanShowAllSubgroups(t *testing.T) {
	days := []ScheduleDay{{
		Date: "2026-04-29T00:00:00Z",
		Subjects: []ScheduleItem{
			{Discipline: "Programming", Subgroup: "left"},
			{Discipline: "Networks", Subgroup: "right"},
		},
	}}

	filtered := FilterScheduleDays(days, "1", true)
	if len(filtered[0].Subjects) != 2 {
		t.Fatalf("unexpected subjects: %#v", filtered[0].Subjects)
	}
}

func TestWeekStartUsesMonday(t *testing.T) {
	loc := time.FixedZone("test", 5*60*60)
	got := WeekStart(time.Date(2026, 4, 30, 15, 0, 0, 0, loc), loc)
	want := time.Date(2026, 4, 27, 6, 0, 0, 0, loc)
	if !got.Equal(want) {
		t.Fatalf("unexpected week start: got %v, want %v", got, want)
	}
}

func TestParseScheduleDate(t *testing.T) {
	loc := time.FixedZone("test", 5*60*60)
	now := time.Date(2026, 4, 30, 15, 0, 0, 0, loc)

	got, err := ParseScheduleDate("01.09", now, loc)
	if err != nil {
		t.Fatal(err)
	}
	want := time.Date(2026, 9, 1, 12, 0, 0, 0, loc)
	if !got.Equal(want) {
		t.Fatalf("unexpected date: got %v, want %v", got, want)
	}

	got, err = ParseScheduleDate("2027-02-03", now, loc)
	if err != nil {
		t.Fatal(err)
	}
	want = time.Date(2027, 2, 3, 12, 0, 0, 0, loc)
	if !got.Equal(want) {
		t.Fatalf("unexpected ISO date: got %v, want %v", got, want)
	}
}

func TestWeekLabelForAcademicYearStart(t *testing.T) {
	loc := time.FixedZone("test", 5*60*60)
	got := WeekLabel(time.Date(2026, 9, 1, 12, 0, 0, 0, loc), loc)
	want := "Неделя 1 (с 31 августа по 05 сентября)"
	if got != want {
		t.Fatalf("unexpected week label: got %q, want %q", got, want)
	}
}

func TestShortDayLabelFormatsDateOnly(t *testing.T) {
	got := ShortDayLabel(ScheduleDay{Date: "2026-05-25", Today: true})
	if got != "Пн 25.05" {
		t.Fatalf("unexpected short day label: %q", got)
	}
}

func TestBuildScheduleURLLeavesTeacherEmptyForTeacherSchedulePath(t *testing.T) {
	client, err := NewClient("https://example.com")
	if err != nil {
		t.Fatal(err)
	}

	rawURL, err := client.buildScheduleURL("/v0/root/tenant/f88efc44efafbd74", 0, "teacher-long-hash-value", 1777240800000)
	if err != nil {
		t.Fatal(err)
	}
	parsedURL, err := url.Parse(rawURL)
	if err != nil {
		t.Fatal(err)
	}

	query := parsedURL.Query()
	if query.Get("Teacher") != "" {
		t.Fatalf("expected empty Teacher query, got %q", query.Get("Teacher"))
	}
	if query.Get("Group") != "" {
		t.Fatalf("expected empty Group query, got %q", query.Get("Group"))
	}
	if query.Get("Week") != "1777240800000" {
		t.Fatalf("unexpected Week query: %q", query.Get("Week"))
	}
	if query.Get("Year") == "" {
		t.Fatal("expected Year query for teacher schedule")
	}
}

func TestFormatScheduleDayShowsTaskAndWebinarOnly(t *testing.T) {
	task := " вебинар в 12-40 "
	webinar := " https://b.ktk-45.ru/rooms/6d0-cn9-gi5-hx4/join "
	item := ScheduleItem{
		Discipline:  "Math",
		LectureHall: 101,
		Pair:        1,
	}
	item.ExtraData.LectureTheme = "Ненужная тема"
	item.ExtraData.LectureHomework = "Ненужное ДЗ"
	item.ExtraData.Homework.Task = &task
	item.ExtraData.Homework.Webinar = &webinar

	text := FormatScheduleDay(ScheduleDay{
		Date:     "2026-04-30T00:00:00Z",
		Subjects: []ScheduleItem{item},
	}, LectureHallMap{})

	if !strings.Contains(text, "Задание: вебинар в 12-40") {
		t.Fatalf("task was not rendered: %q", text)
	}
	if !strings.Contains(text, "Вебинар: https://b.ktk-45.ru/rooms/6d0-cn9-gi5-hx4/join") {
		t.Fatalf("webinar was not rendered: %q", text)
	}
	if strings.Contains(text, "Тема:") || strings.Contains(text, "ДЗ:") || strings.Contains(text, "Ненужная") {
		t.Fatalf("theme or lecture homework leaked into output: %q", text)
	}
}

func TestPairTypeName(t *testing.T) {
	pairTypes := PairTypeMap{
		1:  {ID: 1, Name: "Лекция", BillingType: "Theoretical"},
		3:  {ID: 3, Name: "Экзамен", BillingType: "Certification"},
		5:  {ID: 5, Name: "Практическое занятие", BillingType: "Practice"},
		9:  {ID: 9, Name: "Самостоятельная работа", BillingType: "IndependentWork"},
		10: {ID: 10, Name: "Лабораторная работа", BillingType: "Practice"},
		11: {ID: 11, Name: "Консультация", BillingType: "Consultation"},
	}

	tests := []struct {
		pt    int
		match string
	}{
		{1, "📚 Лекция"},
		{3, "📝 Экзамен"},
		{5, "🔬 Практическое занятие"},
		{9, "📘 Самостоятельная работа"},
		{10, "🔬 Лабораторная работа"},
		{11, "💬 Консультация"},
		{0, ""},
		{99, ""},
	}
	for _, tc := range tests {
		label := pairTypeName(tc.pt, pairTypes)
		if tc.match != "" && label != tc.match {
			t.Fatalf("pairTypeName(%d) = %q, want %q", tc.pt, label, tc.match)
		}
		if label != "" && tc.match == "" {
			t.Fatalf("pairTypeName(%d) = %q, want empty", tc.pt, label)
		}
	}
}

func TestPairTypeNameEmptyMap(t *testing.T) {
	if label := pairTypeName(1, nil); label != "" {
		t.Fatalf("pairTypeName with nil map = %q, want empty", label)
	}
	if label := pairTypeName(1, PairTypeMap{}); label != "" {
		t.Fatalf("pairTypeName with empty map = %q, want empty", label)
	}
}

func FuzzParseScheduleDate(f *testing.F) {
	loc := time.FixedZone("test", 5*60*60)
	now := time.Date(2026, 4, 30, 15, 0, 0, 0, loc)

	f.Add("01.09")
	f.Add("2026-09-01")
	f.Add("31.12.2026")
	f.Add("1.1.2026")
	f.Add("")
	f.Add("32.13")
	f.Add("not-a-date")
	f.Add("2026-02-29")

	f.Fuzz(func(t *testing.T, input string) {
		_, _ = ParseScheduleDate(input, now, loc)
	})
}

func FuzzNormalizeSubgroup(f *testing.F) {
	f.Add("left")
	f.Add("right")
	f.Add("1")
	f.Add("2")
	f.Add("первая")
	f.Add("вторая")
	f.Add("общая")
	f.Add("both")
	f.Add("LEFT")
	f.Add("  left  ")
	f.Add("1подгруппа")
	f.Add("подгруппа_2")
	f.Add("middle")
	f.Add("common")
	f.Add("")

	f.Fuzz(func(t *testing.T, input string) {
		result := normalizeSubgroup(input)
		if result == "" && input != "" {
			t.Fatalf("normalizeSubgroup returned empty for %q", input)
		}
	})
}
