package ktk

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"time"
)

const pairTypeIndependentWork = 9
const maxResponseBodyBytes = 4 << 20

var shortWeekdayNames = [...]string{"Вс", "Пн", "Вт", "Ср", "Чт", "Пт", "Сб"}

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

type lectureHallResponse struct {
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
	if c.endpointSnapshot().SchedulePath == "" {
		if err := c.RefreshEndpoints(ctx, groupID, weekMillis); err != nil {
			return nil, err
		}
		return c.getSchedule(ctx, groupID, weekMillis)
	}

	days, err := c.getSchedule(ctx, groupID, weekMillis)
	if err == nil {
		return days, nil
	}
	if !shouldRefreshEndpoints(err) {
		return nil, err
	}

	if refreshErr := c.RefreshEndpoints(ctx, groupID, weekMillis); refreshErr != nil {
		return nil, fmt.Errorf("%w; endpoint refresh failed: %v", err, refreshErr)
	}

	return c.getSchedule(ctx, groupID, weekMillis)
}

func (c *Client) getSchedule(ctx context.Context, groupID int, weekMillis int64) ([]ScheduleDay, error) {
	endpoint := c.endpointSnapshot()
	requestURL, err := c.buildScheduleURL(endpoint.SchedulePath, groupID, weekMillis)
	if err != nil {
		return nil, err
	}

	req, err := http.NewRequestWithContext(ctx, http.MethodGet, requestURL, nil)
	if err != nil {
		return nil, err
	}

	req.Header.Set("Accept", "*/*")
	req.Header.Set("Referer", c.baseURL+"/")
	req.Header.Set("User-Agent", "ktk-schedule/1.0")

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	body, _ := readLimitedBody(resp)

	if resp.StatusCode != http.StatusOK {
		return nil, endpointError{operation: "schedule", statusCode: resp.StatusCode, status: resp.Status, body: string(body)}
	}

	var days []ScheduleDay
	if err := json.Unmarshal(body, &days); err != nil {
		return nil, endpointError{operation: "schedule", statusCode: resp.StatusCode, status: resp.Status, body: string(body), err: err}
	}

	return days, nil
}

func (c *Client) GetLectureHalls(ctx context.Context, groupID int, weekMillis int64) (LectureHallMap, error) {
	endpoint := c.endpointSnapshot()
	if endpoint.LectureHallPath == "" || endpoint.BranchID == "" {
		if err := c.RefreshEndpoints(ctx, groupID, weekMillis); err != nil {
			return nil, err
		}
		return c.getLectureHalls(ctx)
	}

	halls, err := c.getLectureHalls(ctx)
	if err == nil {
		return halls, nil
	}
	if !shouldRefreshEndpoints(err) {
		return nil, err
	}

	if refreshErr := c.RefreshEndpoints(ctx, groupID, weekMillis); refreshErr != nil {
		return nil, fmt.Errorf("%w; endpoint refresh failed: %v", err, refreshErr)
	}

	return c.getLectureHalls(ctx)
}

func (c *Client) getLectureHalls(ctx context.Context) (LectureHallMap, error) {
	endpoint := c.endpointSnapshot()
	requestURL, err := c.buildLectureHallURL(endpoint.LectureHallPath, endpoint.BranchID)
	if err != nil {
		return nil, err
	}

	req, err := http.NewRequestWithContext(ctx, http.MethodGet, requestURL, nil)
	if err != nil {
		return nil, err
	}

	req.Header.Set("Accept", "*/*")
	req.Header.Set("Referer", c.baseURL+"/")
	req.Header.Set("User-Agent", "ktk-schedule/1.0")

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	body, _ := readLimitedBody(resp)

	if resp.StatusCode != http.StatusOK {
		return nil, endpointError{operation: "lecture halls", statusCode: resp.StatusCode, status: resp.Status, body: string(body)}
	}

	var data lectureHallResponse
	if err := json.Unmarshal(body, &data); err != nil {
		return nil, endpointError{operation: "lecture halls", statusCode: resp.StatusCode, status: resp.Status, body: string(body), err: err}
	}

	result := make(LectureHallMap)

	for _, halls := range data.LectureHalls {
		for _, hall := range halls {
			result[hall.ID] = hall
		}
	}

	return result, nil
}

func (c *Client) buildScheduleURL(path string, groupID int, weekMillis int64) (string, error) {
	requestURL, err := c.resolveURL(path)
	if err != nil {
		return "", err
	}

	parsedURL, err := url.Parse(requestURL)
	if err != nil {
		return "", err
	}

	query := parsedURL.Query()
	query.Set("Teacher", "")
	query.Set("Group", strconv.Itoa(groupID))
	query.Set("Week", strconv.FormatInt(weekMillis, 10))
	parsedURL.RawQuery = query.Encode()

	return parsedURL.String(), nil
}

func (c *Client) buildLectureHallURL(path, branchID string) (string, error) {
	requestURL, err := c.resolveURL(path)
	if err != nil {
		return "", err
	}

	parsedURL, err := url.Parse(requestURL)
	if err != nil {
		return "", err
	}

	query := parsedURL.Query()
	query.Set("Branch", branchID)
	parsedURL.RawQuery = query.Encode()

	return parsedURL.String(), nil
}

func readLimitedBody(resp *http.Response) ([]byte, error) {
	return io.ReadAll(io.LimitReader(resp.Body, maxResponseBodyBytes))
}

type endpointError struct {
	operation  string
	statusCode int
	status     string
	body       string
	err        error
}

func (e endpointError) Error() string {
	if e.err != nil {
		return fmt.Sprintf("%s failed: unexpected response: %v", e.operation, e.err)
	}
	return fmt.Sprintf("%s failed: %s", e.operation, e.status)
}

func (e endpointError) Unwrap() error {
	return e.err
}

func shouldRefreshEndpoints(err error) bool {
	var endpointErr endpointError
	if !errors.As(err, &endpointErr) {
		return false
	}

	if endpointErr.statusCode == http.StatusNotFound || endpointErr.statusCode == http.StatusGone {
		return true
	}
	if endpointErr.err != nil && looksLikeHTML(endpointErr.body) {
		return true
	}

	return false
}

func looksLikeHTML(body string) bool {
	body = strings.TrimSpace(strings.ToLower(body))
	return strings.HasPrefix(body, "<!doctype html") || strings.HasPrefix(body, "<html")
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

func FindDateIndex(days []ScheduleDay, now time.Time, loc *time.Location) int {
	if loc == nil {
		loc = time.Local
	}

	today := now.In(loc).Format(time.DateOnly)
	for i, day := range days {
		if scheduleDate(day.Date, loc) == today {
			return i
		}
	}

	return FindTodayIndex(days)
}

func scheduleDate(value string, loc *time.Location) string {
	parsed, err := time.Parse(time.RFC3339, value)
	if err != nil {
		value = strings.TrimSpace(value)
		if len(value) >= len(time.DateOnly) {
			return value[:len(time.DateOnly)]
		}
		return value
	}

	return parsed.In(loc).Format(time.DateOnly)
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
		isIndependent := subject.ExtendedData.PairType == pairTypeIndependentWork
		hall := FormatLectureHall(subject.LectureHall, halls)

		b.WriteString(strconv.Itoa(subject.Pair))
		b.WriteString(" пара — ")
		b.WriteString(subject.Discipline)
		b.WriteByte('\n')

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
	if ok && hall.Number != "" {
		return hall.Number
	}

	return strconv.Itoa(id)
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

	return shortWeekdayNames[parsed.Weekday()] + " " + date
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
