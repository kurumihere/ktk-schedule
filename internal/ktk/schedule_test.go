package ktk

import (
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
