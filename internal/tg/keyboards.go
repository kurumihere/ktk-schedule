package tg

import (
	"fmt"

	"github.com/go-telegram/bot/models"

	"ktk-schedule/internal/ktk"
)

func ScheduleKeyboard(days []ktk.ScheduleDay, currentIndex int) *models.InlineKeyboardMarkup {
	rows := make([][]models.InlineKeyboardButton, 0, len(days)+1)

	prevText := "⬅️"
	nextText := "➡️"

	if currentIndex <= 0 {
		prevText = "⛔"
	}

	if currentIndex >= len(days)-1 {
		nextText = "⛔"
	}

	rows = append(rows, []models.InlineKeyboardButton{
		{Text: prevText, CallbackData: "schedule:prev"},
		{Text: "Сегодня", CallbackData: "schedule:today"},
		{Text: nextText, CallbackData: "schedule:next"},
	})

	for i, day := range days {
		label := ktk.ShortDayLabel(day)
		if i == currentIndex {
			label = "✅ " + label
		}

		rows = append(rows, []models.InlineKeyboardButton{
			{Text: label, CallbackData: fmt.Sprintf("schedule:day:%d", i)},
		})
	}

	return &models.InlineKeyboardMarkup{InlineKeyboard: rows}
}
