package ktk

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"
)

const PairTypeIndependentWork = 9

type ScheduleDay struct {
	Date     string         `json:"Date"`
	Today    bool           `json:"Today"`
	MaxPair  int            `json:"MaxPair"`
	Subjects []ScheduleItem `json:"Subjects"`
}

type ScheduleItem struct {
	Discipline   string `json:"Discipline"`
	Teacher      string `json:"Teacher"`
	LectureHall  int    `json:"LectureHall"`
	Pair         int    `json:"Pair"`
	ExtendedData struct {
		AcademicHour   int    `json:"AcademicHour"`
		DisciplineFull string `json:"DisciplineFull"`
		PairType       int    `json:"PairType"`
	} `json:"ExtendedData"`
	ExtraData struct {
		LectureTheme    string `json:"LectureTheme"`
		LectureHomework string `json:"LectureHomework"`
		Homework        struct {
			Task     *string `json:"Task"`
			Deadline *string `json:"Deadline"`
		} `json:"Homework"`
	} `json:"ExtraData"`
}

type LectureHallResponse struct {
	LectureHalls map[string][]LectureHall `json:"LectureHalls"`
}

type LectureHall struct {
	ID      int    `json:"ID"`
	Housing int    `json:"Housing"`
	Level   int    `json:"Level"`
	Number  string `json:"Number"`
	Virtual bool   `json:"Virtual"`
}

type LectureHallMap map[int]LectureHall

func (c *Client) GetSchedule(ctx context.Context, groupID int, weekMillis int64) ([]ScheduleDay, error) {
	url := fmt.Sprintf(
		"%s/v0/050ba56b37f0337e/8f7c3cbf3115ae2c/28beedf026903f63?Teacher=&Group=%d&Week=%d",
		c.baseURL,
		groupID,
		weekMillis,
	)

	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return nil, err
	}

	req.Header.Set("Accept", "*/*")
	req.Header.Set("Referer", c.baseURL+"/")
	req.Header.Set("User-Agent", "ktk-schedule/1.0")

	resp, err := c.http.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	body, _ := io.ReadAll(resp.Body)

	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("schedule failed: %s: %s", resp.Status, string(body))
	}

	var days []ScheduleDay
	if err := json.Unmarshal(body, &days); err != nil {
		return nil, err
	}

	return days, nil
}

func (c *Client) GetLectureHalls(ctx context.Context) (LectureHallMap, error) {
	url := c.baseURL + "/v0/050ba56b37f0337e/8f7c3cbf3115ae2c/lecture-hall?Branch=44b72cd889a44234a8a7a49750fedf1bc5a654d8b06257ec5866711be0be6286"
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return nil, err
	}

	req.Header.Set("Accept", "*/*")
	req.Header.Set("Referer", c.baseURL+"/")
	req.Header.Set("User-Agent", "ktk-schedule/1.0")

	resp, err := c.http.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	body, _ := io.ReadAll(resp.Body)

	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("lecture halls failed: %s: %s", resp.Status, string(body))
	}

	var data LectureHallResponse
	if err := json.Unmarshal(body, &data); err != nil {
		return nil, err
	}

	result := make(LectureHallMap)

	for _, halls := range data.LectureHalls {
		for _, hall := range halls {
			result[hall.ID] = hall
		}
	}

	return result, nil
}

func WeekStartMillis(now time.Time, loc *time.Location) int64 {
	t := now.In(loc)

	weekday := int(t.Weekday())
	if weekday == 0 {
		weekday = 7
	}

	monday := time.Date(
		t.Year(),
		t.Month(),
		t.Day(),
		6, 0, 0, 0,
		loc,
	).AddDate(0, 0, -(weekday - 1))

	return monday.UnixMilli()
}

func FindTodayIndex(days []ScheduleDay) int {
	for i, day := range days {
		if day.Today {
			return i
		}
	}
	return 0
}

func FormatScheduleDay(day ScheduleDay, halls LectureHallMap) string {
	var b strings.Builder

	title := FormatDate(day.Date)
	if day.Today {
		b.WriteString("📅 Сегодня — " + title + "\n\n")
	} else {
		b.WriteString("📅 " + title + "\n\n")
	}

	if len(day.Subjects) == 0 {
		b.WriteString("Пар нет.")
		return b.String()
	}

	for _, subject := range day.Subjects {
		isIndependent := subject.ExtendedData.PairType == PairTypeIndependentWork
		hall := FormatLectureHall(subject.LectureHall, halls)

		b.WriteString(fmt.Sprintf("%d пара — %s\n", subject.Pair, subject.Discipline))

		if isIndependent {
			b.WriteString("📘 Тип: Самостоятельная работа\n")
		}

		if subject.Teacher != "" {
			b.WriteString("👤 " + subject.Teacher + "\n")
		}

		b.WriteString("🏫 Кабинет: " + hall + "\n")

		if subject.ExtraData.LectureTheme != "" && subject.ExtraData.LectureTheme != "*нераспределенное занятие" {
			b.WriteString("Тема: " + subject.ExtraData.LectureTheme + "\n")
		}

		if subject.ExtraData.LectureHomework != "" {
			b.WriteString("ДЗ: " + subject.ExtraData.LectureHomework + "\n")
		}

		if subject.ExtraData.Homework.Task != nil && *subject.ExtraData.Homework.Task != "" {
			b.WriteString("Задание: " + *subject.ExtraData.Homework.Task + "\n")
		}

		b.WriteString("\n")
	}

	return strings.TrimSpace(b.String())
}

func FormatLectureHall(id int, halls LectureHallMap) string {
	hall, ok := halls[id]
	if !ok {
		return fmt.Sprintf("%d", id)
	}

	if hall.Number == "" {
		return fmt.Sprintf("%d", id)
	}

	if hall.Virtual {
		return hall.Number
	}

	return hall.Number
}

func ShortDayLabel(day ScheduleDay) string {
	date := FormatShortDate(day.Date)

	if day.Today {
		return "Сегодня " + date
	}

	parsed, err := time.Parse(time.RFC3339, day.Date)
	if err != nil {
		return date
	}

	names := map[time.Weekday]string{
		time.Monday:    "Пн",
		time.Tuesday:   "Вт",
		time.Wednesday: "Ср",
		time.Thursday:  "Чт",
		time.Friday:    "Пт",
		time.Saturday:  "Сб",
		time.Sunday:    "Вс",
	}

	return names[parsed.Weekday()] + " " + date
}

func FormatDate(value string) string {
	t, err := time.Parse(time.RFC3339, value)
	if err != nil {
		return strings.TrimSuffix(value, "T00:00:00Z")
	}
	return t.Format("02.01.2006")
}

func FormatShortDate(value string) string {
	t, err := time.Parse(time.RFC3339, value)
	if err != nil {
		return strings.TrimSuffix(value, "T00:00:00Z")
	}
	return t.Format("02.01")
}
