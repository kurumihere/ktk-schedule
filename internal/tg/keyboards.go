package tg

import (
	"fmt"

	tgbotapi "github.com/go-telegram-bot-api/telegram-bot-api/v5"

	"ktk-schedule/internal/ktk"
)

func ScheduleKeyboard(days []ktk.ScheduleDay, currentIndex int) *tgbotapi.InlineKeyboardMarkup {
	rows := make([][]tgbotapi.InlineKeyboardButton, 0)

	prevText := "⬅️"
	nextText := "➡️"

	if currentIndex <= 0 {
		prevText = "⛔"
	}

	if currentIndex >= len(days)-1 {
		nextText = "⛔"
	}

	rows = append(rows, tgbotapi.NewInlineKeyboardRow(
		tgbotapi.NewInlineKeyboardButtonData(prevText, "schedule:prev"),
		tgbotapi.NewInlineKeyboardButtonData("Сегодня", "schedule:today"),
		tgbotapi.NewInlineKeyboardButtonData(nextText, "schedule:next"),
	))

	for i, day := range days {
		label := ktk.ShortDayLabel(day)
		if i == currentIndex {
			label = "✅ " + label
		}

		rows = append(rows, tgbotapi.NewInlineKeyboardRow(
			tgbotapi.NewInlineKeyboardButtonData(label, fmt.Sprintf("schedule:day:%d", i)),
		))
	}

	keyboard := tgbotapi.NewInlineKeyboardMarkup(rows...)
	return &keyboard
}
