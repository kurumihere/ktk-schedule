package tg

import (
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

	kb := ScheduleKeyboard(days, 0, weekStart, loc)
	for _, row := range kb.InlineKeyboard {
		for _, btn := range row {
			if len(btn.CallbackData) > maxCallbackData {
				t.Errorf("callback data too long: %s (%d bytes)", btn.CallbackData, len(btn.CallbackData))
			}
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
