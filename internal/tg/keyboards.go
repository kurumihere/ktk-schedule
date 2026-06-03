package tg

import (
	"fmt"
	"time"

	"github.com/go-telegram/bot/models"

	"ktk-schedule/internal/ktk"
)

const maxCallbackData = 64

func callbackDataOrPanic(format string, args ...any) string {
	data := fmt.Sprintf(format, args...)
	if len(data) > maxCallbackData {
		panic(fmt.Sprintf("callback data too long (%d bytes): %s", len(data), data))
	}
	return data
}

func ScheduleKeyboard(days []ktk.ScheduleDay, currentIndex int, weekStart time.Time, loc *time.Location, fileCount int, viewingGroupID int, isTeacher bool, subgroup string, showAllSubgroups bool) *models.InlineKeyboardMarkup {
	if weekStart.IsZero() {
		weekStart = ktk.WeekStart(time.Now(), loc)
	}

	rows := make([][]models.InlineKeyboardButton, 0, len(days)+2)
	rows = append(rows, []models.InlineKeyboardButton{
		{Text: "⬅️ неделя", CallbackData: "schedule:week:prev"},
		{Text: ktk.WeekLabel(weekStart, loc), CallbackData: "schedule:week:select"},
		{Text: "неделя ➡️", CallbackData: "schedule:week:next"},
	})

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

	if viewingGroupID > 0 {
		myLabel := "🏠 Своя группа"
		if isTeacher {
			myLabel = "👤 Своё расписание"
		}
		rows = append(rows, []models.InlineKeyboardButton{
			{Text: myLabel, CallbackData: "schedule:my"},
			{Text: "🔍 Другая группа", CallbackData: "schedule:group:select"},
		})
	} else {
		rows = append(rows, []models.InlineKeyboardButton{
			{Text: "🔍 Расписание группы", CallbackData: "schedule:group:select"},
		})
	}

	if !isTeacher {
		rows = append(rows, subgroupRow(subgroup, showAllSubgroups))
	}

	if fileCount > 0 {
		label := fmt.Sprintf("📎 Скачать файлы (%d)", fileCount)
		rows = append(rows, []models.InlineKeyboardButton{
			{Text: label, CallbackData: "schedule:download"},
		})
	}

	for i, day := range days {
		label := ktk.ShortDayLabel(day)
		if i == currentIndex {
			label = "✅ " + label
		}

		rows = append(rows, []models.InlineKeyboardButton{
			{Text: label, CallbackData: callbackDataOrPanic("schedule:day:%d", i)},
		})
	}

	return &models.InlineKeyboardMarkup{InlineKeyboard: rows}
}

func subgroupRow(subgroup string, showAll bool) []models.InlineKeyboardButton {
	btn1 := models.InlineKeyboardButton{Text: "1 подгруппа", CallbackData: "schedule:subgroup:left"}
	btn2 := models.InlineKeyboardButton{Text: "2 подгруппа", CallbackData: "schedule:subgroup:right"}
	btnAll := models.InlineKeyboardButton{Text: "Обе", CallbackData: "schedule:subgroup:all"}

	switch {
	case showAll:
		btnAll.Text = "✅ Обе"
	case subgroup == "right":
		btn2.Text = "✅ 2 подгруппа"
	default:
		btn1.Text = "✅ 1 подгруппа"
	}

	return []models.InlineKeyboardButton{btn1, btn2, btnAll}
}

func WeekSelectKeyboard(baseWeekStart time.Time, offset int, loc *time.Location) *models.InlineKeyboardMarkup {
	if baseWeekStart.IsZero() {
		baseWeekStart = ktk.WeekStart(time.Now(), loc)
	}

	center := ktk.WeekStart(baseWeekStart.AddDate(0, 0, offset*7), loc)
	selectedWeekStart := ktk.WeekStart(baseWeekStart, loc)

	rows := make([][]models.InlineKeyboardButton, 0, 7)
	rows = append(rows, []models.InlineKeyboardButton{
		{Text: "⬅️", CallbackData: "schedule:week:page:-1"},
		{Text: "Назад", CallbackData: "schedule:back"},
		{Text: "➡️", CallbackData: "schedule:week:page:1"},
	})

	for i := -2; i <= 2; i++ {
		weekStart := center.AddDate(0, 0, i*7)
		label := ktk.WeekLabel(weekStart, loc)
		if weekStart.Equal(selectedWeekStart) {
			label = "✅ " + label
		}

		rows = append(rows, []models.InlineKeyboardButton{
			{Text: label, CallbackData: callbackDataOrPanic("schedule:week:open:%d", weekStart.UnixMilli())},
		})
	}

	rows = append(rows, []models.InlineKeyboardButton{
		{Text: "Текущая неделя", CallbackData: "schedule:week:today"},
	})

	return &models.InlineKeyboardMarkup{InlineKeyboard: rows}
}
