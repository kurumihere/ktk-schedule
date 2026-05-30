package tg

import (
	"strings"
	"testing"
	"time"

	"ktk-schedule/internal/ktk"
)

func TestCallbackDataLength(t *testing.T) {
	loc := time.UTC
	days := []ktk.ScheduleDay{
		{Date: "2026-05-14T00:00:00Z", Subjects: []ktk.ScheduleItem{
			{Discipline: "Math", Pair: 1},
		}},
	}
	weekStart := time.Date(2026, 5, 11, 0, 0, 0, 0, loc)

	kb := ScheduleKeyboard(days, 0, weekStart, loc, 0)
	for _, row := range kb.InlineKeyboard {
		for _, btn := range row {
			if len(btn.CallbackData) > maxCallbackData {
				t.Errorf("callback data too long: %s (%d bytes)", btn.CallbackData, len(btn.CallbackData))
			}
		}
	}
}

func TestScheduleKeyboardNoSelectedDay(t *testing.T) {
	loc := time.UTC
	days := []ktk.ScheduleDay{
		{Date: "2026-06-01", Subjects: []ktk.ScheduleItem{{Discipline: "Math", Pair: 1}}},
		{Date: "2026-06-02", Subjects: []ktk.ScheduleItem{{Discipline: "Physics", Pair: 1}}},
	}

	kb := ScheduleKeyboard(days, -1, time.Date(2026, 6, 1, 0, 0, 0, 0, loc), loc, 0)

	if kb.InlineKeyboard[1][0].Text != "⛔" {
		t.Fatalf("expected previous button to be disabled, got %q", kb.InlineKeyboard[1][0].Text)
	}
	if kb.InlineKeyboard[1][2].Text != "➡️" {
		t.Fatalf("expected next button to stay enabled, got %q", kb.InlineKeyboard[1][2].Text)
	}
	for _, row := range kb.InlineKeyboard[2:] {
		if strings.HasPrefix(row[0].Text, "✅ ") {
			t.Fatalf("did not expect selected day label, got %q", row[0].Text)
		}
	}
}

func TestWeekSelectCallbackDataLength(t *testing.T) {
	loc := time.UTC
	weekStart := time.Date(2026, 5, 11, 0, 0, 0, 0, loc)

	kb := WeekSelectKeyboard(weekStart, 0, loc)
	for _, row := range kb.InlineKeyboard {
		for _, btn := range row {
			if len(btn.CallbackData) > maxCallbackData {
				t.Errorf("callback data too long: %s (%d bytes)", btn.CallbackData, len(btn.CallbackData))
			}
		}
	}
}

func TestCallbackDataOrPanic(t *testing.T) {
	defer func() {
		if r := recover(); r != nil {
			t.Fatalf("unexpected panic: %v", r)
		}
	}()

	data := callbackDataOrPanic("schedule:day:%d", 5)
	if data != "schedule:day:5" {
		t.Errorf("expected schedule:day:5, got %s", data)
	}
}

func TestCallbackDataOrPanicTooLong(t *testing.T) {
	defer func() {
		if r := recover(); r == nil {
			t.Fatal("expected panic for too long callback data")
		}
	}()

	long := "x"
	for len(long) < 70 {
		long += "x"
	}
	callbackDataOrPanic("prefix:%s", long)
}
