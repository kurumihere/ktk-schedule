package ktk

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"time"
)

func pairTypeName(pt int, pairTypes PairTypeMap) string {
	if p, ok := pairTypes[pt]; ok {
		return pairTypeEmoji(p.BillingType) + p.Name
	}
	return ""
}

func pairTypeEmoji(billingType string) string {
	switch billingType {
	case "Theoretical":
		return "📚 "
	case "Practice":
		return "🔬 "
	case "IndependentWork":
		return "📘 "
	case "Certification":
		return "📝 "
	case "Consultation":
		return "💬 "
	case "CourseWork":
		return "📄 "
	default:
		return ""
	}
}

const (
	maxResponseBodyBytes = 4 << 20
	maxDownloadBodyBytes = 50 << 20
)
const maxDebugScheduleItemBytes = 4096

var shortWeekdayNames = [...]string{"Вс", "Пн", "Вт", "Ср", "Чт", "Пт", "Сб"}

var MonthGenitive = [...]string{
	"",
	"января",
	"февраля",
	"марта",
	"апреля",
	"мая",
	"июня",
	"июля",
	"августа",
	"сентября",
	"октября",
	"ноября",
	"декабря",
}

type ScheduleDay struct {
	CallPreset int            `json:"CallPreset"`
	Date       string         `json:"Date"`
	Today      bool           `json:"Today"`
	MaxPair    int            `json:"MaxPair"`
	Subjects   []ScheduleItem `json:"Subjects"`
}

type ScheduleItem struct {
	Appraisal    int    `json:"Appraisal"`
	Discipline   string `json:"Discipline"`
	LectureHall  int    `json:"LectureHall"`
	Mark         int    `json:"Mark"`
	Pair         int    `json:"Pair"`
	Subgroup     string `json:"Subgroup"`
	Teacher      string `json:"Teacher"`
	Group        string `json:"Group"`
	CallPreset   int    `json:"CallPreset"`
	ExtendedData struct {
		AcademicHour   int    `json:"AcademicHour"`
		DisciplineFull string `json:"DisciplineFull"`
		PairType       int    `json:"PairType"`
	} `json:"ExtendedData"`
	ExtraData struct {
		LectureTheme    string `json:"LectureTheme"`
		LectureHomework string `json:"LectureHomework"`
		LectureType     int    `json:"LectureType"`
		Sheet           int    `json:"Sheet"`
		Homework        struct {
			Task       *string `json:"Task"`
			Deadline   *string `json:"Deadline"`
			Webinar    *string `json:"Webinar"`
			Files      []int   `json:"Files"`
			LockUpload *bool   `json:"LockUpload"`
		} `json:"Homework"`
	} `json:"ExtraData"`
}

type scheduleV3Wrapper struct {
	Branch  string  `json:"Branch"`
	DayList []dayV3 `json:"DayList"`
	MaxPair int     `json:"MaxPair"`
}

type dayV3 struct {
	Date  string   `json:"Date"`
	Pairs []pairV3 `json:"Pairs"`
	Today bool     `json:"Today"`
}

type pairV3 struct {
	Number    int                       `json:"Number"`
	Subgroups map[string][]ScheduleItem `json:"Subgroups"`
}

type CallSetItem struct {
	Break      int `json:"Break"`
	Duration   int `json:"Duration"`
	PairNumber int `json:"PairNumber"`
}

type CallPreset struct {
	ID      int           `json:"ID"`
	Name    string        `json:"Name"`
	Begin   string        `json:"Begin"`
	CallSet []CallSetItem `json:"CallSet"`
}

type CallPresetMap map[int]CallPreset

type PairTiming struct {
	StartHour int
	StartMin  int
	EndHour   int
	EndMin    int
	Duration  int
}

type FormatOptions struct {
	ShowSubgroupLabels bool
	CallPresets        CallPresetMap
	AbsenceMarks       []AbsenceMark
	AbsenceByDigit     map[int]string
	PairTypes          PairTypeMap
	FileNames          map[int]string
	StudentFiles       map[int]int
	Loc                *time.Location
	Now                time.Time
	IsTeacher          bool
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

type AbsenceMark struct {
	Caption string `json:"Caption"`
	Digit   int    `json:"Digit"`
}

type PairType struct {
	ID          int    `json:"ID"`
	Name        string `json:"Name"`
	ShortName   string `json:"ShortName"`
	Code        int    `json:"Code"`
	BillingType string `json:"BillingType"`
}

type PairTypeMap map[int]PairType

const (
	markGradePlus  = 128
	markGradeMinus = 256
	markMissing    = 16
	markLate       = 32
	markBug        = 8
)

var sickMarkDigits = map[int]bool{
	4:  true,
	11: true,
}

var markSymbols = map[int]string{
	markGradePlus:  "+",
	markGradeMinus: "-",
	markMissing:    "Н",
	markLate:       "О",
	markBug:        "🪲",
}

func (c *Client) GetSchedule(ctx context.Context, groupID int, weekMillis int64) ([]ScheduleDay, error) {
	endpoint := c.endpointSnapshot()
	if endpoint.SchedulePath == "" {
		if err := c.RefreshEndpoints(ctx, groupID, weekMillis); err != nil {
			return nil, err
		}
		return c.getSchedule(ctx, c.endpointSnapshot().SchedulePath, groupID, "", weekMillis)
	}

	days, err := c.getSchedule(ctx, endpoint.SchedulePath, groupID, "", weekMillis)
	if err == nil {
		return days, nil
	}
	if !shouldRefreshEndpoints(err) {
		return nil, err
	}

	if refreshErr := c.RefreshEndpoints(ctx, groupID, weekMillis); refreshErr != nil {
		return nil, fmt.Errorf("%w; endpoint refresh failed: %v", err, refreshErr)
	}

	return c.getSchedule(ctx, c.endpointSnapshot().SchedulePath, groupID, "", weekMillis)
}

func (c *Client) GetTeacherSchedule(ctx context.Context, teacherHash string, weekMillis int64) ([]ScheduleDay, error) {
	if c.teacherScheduleHash != "" {
		endpoint := c.endpointSnapshot()
		basePath := trimLastSegment(endpoint.SchedulePath)
		if basePath == "" {
			basePath = trimLastSegment(endpoint.CallPresetPath)
		}
		if basePath != "" {
			teacherPath := basePath + "/" + c.teacherScheduleHash
			days, err := c.getSchedule(ctx, teacherPath, 0, teacherHash, weekMillis)
			if err == nil {
				return days, nil
			}
		}
	}

	path := c.endpointSnapshot().SchedulePath
	if path != "" {
		days, err := c.getSchedule(ctx, path, 0, teacherHash, weekMillis)
		if err == nil {
			return days, nil
		}
	}

	if err := c.RefreshEndpoints(ctx, 0, weekMillis, teacherHash); err != nil {
		return nil, err
	}
	path = c.endpointSnapshot().SchedulePath
	if path == "" {
		return nil, fmt.Errorf("teacher schedule endpoint not found")
	}

	return c.getSchedule(ctx, path, 0, teacherHash, weekMillis)
}

func parseScheduleDays(body []byte) ([]ScheduleDay, error) {
	var days []ScheduleDay
	if err := json.Unmarshal(body, &days); err != nil || len(days) == 0 || days[0].Date == "" {
		var wrapper []scheduleV3Wrapper
		if uerr := json.Unmarshal(body, &wrapper); uerr != nil || len(wrapper) == 0 {
			if err != nil {
				return nil, err
			}
			return nil, fmt.Errorf("empty schedule")
		}
		days = convertV3Schedule(wrapper)
	}
	return days, nil
}

func convertV3Schedule(wrapper []scheduleV3Wrapper) []ScheduleDay {
	if len(wrapper) == 0 {
		return nil
	}
	w := wrapper[0]
	days := make([]ScheduleDay, 0, len(w.DayList))
	for _, d := range w.DayList {
		day := ScheduleDay{
			Date:    d.Date,
			Today:   d.Today,
			MaxPair: w.MaxPair,
		}
		var callPreset int
		for _, p := range d.Pairs {
			for subgroupKey, items := range p.Subgroups {
				for _, item := range items {
					s := item
					s.Pair = p.Number
					s.Subgroup = subgroupKey
					if callPreset == 0 && s.CallPreset != 0 {
						callPreset = s.CallPreset
					}
					day.Subjects = append(day.Subjects, s)
				}
			}
		}
		day.CallPreset = callPreset
		days = append(days, day)
	}
	if days == nil {
		return []ScheduleDay{}
	}
	return days
}

func (c *Client) getSchedule(ctx context.Context, path string, groupID int, teacherHash string, weekMillis int64) ([]ScheduleDay, error) {
	requestURL, err := c.buildScheduleURL(path, groupID, teacherHash, weekMillis)
	if err != nil {
		return nil, err
	}

	var days []ScheduleDay
	err = retryGet(ctx, 3, func(retryCtx context.Context) error {
		req, err := http.NewRequestWithContext(retryCtx, http.MethodGet, requestURL, nil)
		if err != nil {
			return err
		}
		req.Header.Set("Accept", "*/*")
		req.Header.Set("Referer", c.baseURL+"/")
		req.Header.Set("User-Agent", "ktk-schedule/1.0")

		resp, err := c.httpClient.Do(req)
		if err != nil {
			return err
		}
		defer resp.Body.Close()

		body, _ := readLimitedBody(resp)

		if resp.StatusCode != http.StatusOK {
			return endpointError{operation: "schedule", statusCode: resp.StatusCode, status: resp.Status, body: string(body)}
		}
		if c.debugSchedule {
			logScheduleDebug(body)
		}

		result, parseErr := parseScheduleDays(body)
		if parseErr != nil {
			return endpointError{operation: "schedule", statusCode: resp.StatusCode, status: resp.Status, body: string(body), err: parseErr}
		}
		days = result
		return nil
	})

	return days, err
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

	var result LectureHallMap
	err = retryGet(ctx, 3, func(retryCtx context.Context) error {
		req, err := http.NewRequestWithContext(retryCtx, http.MethodGet, requestURL, nil)
		if err != nil {
			return err
		}
		req.Header.Set("Accept", "*/*")
		req.Header.Set("Referer", c.baseURL+"/")
		req.Header.Set("User-Agent", "ktk-schedule/1.0")

		resp, err := c.httpClient.Do(req)
		if err != nil {
			return err
		}
		defer resp.Body.Close()

		body, _ := readLimitedBody(resp)

		if resp.StatusCode != http.StatusOK {
			return endpointError{operation: "lecture halls", statusCode: resp.StatusCode, status: resp.Status, body: string(body)}
		}

		var data lectureHallResponse
		if err := json.Unmarshal(body, &data); err != nil {
			return endpointError{operation: "lecture halls", statusCode: resp.StatusCode, status: resp.Status, body: string(body), err: err}
		}

		halls := make(LectureHallMap)
		for _, hallsList := range data.LectureHalls {
			for _, hall := range hallsList {
				halls[hall.ID] = hall
			}
		}
		result = halls
		return nil
	})

	return result, err
}

func (c *Client) GetCallPresets(ctx context.Context) (CallPresetMap, error) {
	endpoint := c.endpointSnapshot()
	path := endpoint.CallPresetPath
	if path == "" {
		path = DeriveCallPresetPath(endpoint.SchedulePath)
	}
	if path == "" {
		return nil, fmt.Errorf("call-preset endpoint not available")
	}

	presets, err := c.getCallPresets(ctx, path)
	if err == nil {
		return presets, nil
	}
	if !shouldRefreshEndpoints(err) {
		return nil, err
	}

	if refreshErr := c.RefreshEndpoints(ctx, 0, 0); refreshErr != nil {
		return nil, fmt.Errorf("%w; endpoint refresh failed: %v", err, refreshErr)
	}

	next := c.endpointSnapshot()
	if next.CallPresetPath == "" || next.CallPresetPath == path {
		return nil, err
	}

	return c.getCallPresets(ctx, next.CallPresetPath)
}

func (c *Client) getCallPresets(ctx context.Context, path string) (CallPresetMap, error) {
	var presets []CallPreset
	err := retryGet(ctx, 3, func(retryCtx context.Context) error {
		requestURL, err := c.resolveURL(path)
		if err != nil {
			return err
		}

		req, err := http.NewRequestWithContext(retryCtx, http.MethodGet, requestURL, nil)
		if err != nil {
			return err
		}
		req.Header.Set("Accept", "*/*")
		req.Header.Set("Referer", c.baseURL+"/")
		req.Header.Set("User-Agent", "ktk-schedule/1.0")

		resp, err := c.httpClient.Do(req)
		if err != nil {
			return err
		}
		defer resp.Body.Close()

		body, _ := readLimitedBody(resp)

		if resp.StatusCode != http.StatusOK {
			return endpointError{operation: "call-preset", statusCode: resp.StatusCode, status: resp.Status, body: string(body)}
		}

		var result []CallPreset
		if err := json.Unmarshal(body, &result); err != nil {
			return endpointError{operation: "call-preset", statusCode: resp.StatusCode, status: resp.Status, body: string(body), err: err}
		}
		presets = result
		return nil
	})
	if err != nil {
		return nil, err
	}

	m := make(CallPresetMap, len(presets))
	for _, p := range presets {
		m[p.ID] = p
	}
	return m, nil
}

func (c *Client) GetAbsenceMarks(ctx context.Context) ([]AbsenceMark, error) {
	endpoint := c.endpointSnapshot()
	path := endpoint.AbsenceMarkPath
	if path == "" {
		path = DeriveAbsenceMarkPath(endpoint.SchedulePath)
	}
	if path == "" {
		return nil, fmt.Errorf("absence-mark endpoint not available")
	}

	return c.getAbsenceMarks(ctx, path)
}

func (c *Client) GetPairTypes(ctx context.Context) (PairTypeMap, error) {
	endpoint := c.endpointSnapshot()
	path := endpoint.PairTypePath
	if path == "" {
		return nil, nil
	}

	var types []PairType
	err := retryGet(ctx, 3, func(retryCtx context.Context) error {
		requestURL, err := c.resolveURL(path)
		if err != nil {
			return err
		}

		req, err := http.NewRequestWithContext(retryCtx, http.MethodGet, requestURL, nil)
		if err != nil {
			return err
		}
		req.Header.Set("Accept", "*/*")
		req.Header.Set("Referer", c.baseURL+"/")
		req.Header.Set("User-Agent", "ktk-schedule/1.0")

		resp, err := c.httpClient.Do(req)
		if err != nil {
			return err
		}
		defer resp.Body.Close()

		body, _ := readLimitedBody(resp)

		if resp.StatusCode != http.StatusOK {
			return endpointError{operation: "pair-type", statusCode: resp.StatusCode, status: resp.Status, body: string(body)}
		}

		var result []PairType
		if err := json.Unmarshal(body, &result); err != nil {
			return endpointError{operation: "pair-type", statusCode: resp.StatusCode, status: resp.Status, body: string(body), err: err}
		}
		types = result
		return nil
	})
	if err != nil {
		return nil, err
	}

	m := make(PairTypeMap, len(types))
	for _, pt := range types {
		m[pt.ID] = pt
	}
	return m, nil
}

func (c *Client) getAbsenceMarks(ctx context.Context, path string) ([]AbsenceMark, error) {
	var marks []AbsenceMark
	err := retryGet(ctx, 3, func(retryCtx context.Context) error {
		requestURL, err := c.resolveURL(path)
		if err != nil {
			return err
		}

		req, err := http.NewRequestWithContext(retryCtx, http.MethodGet, requestURL, nil)
		if err != nil {
			return err
		}
		req.Header.Set("Accept", "*/*")
		req.Header.Set("Referer", c.baseURL+"/")
		req.Header.Set("User-Agent", "ktk-schedule/1.0")

		resp, err := c.httpClient.Do(req)
		if err != nil {
			return err
		}
		defer resp.Body.Close()

		body, _ := readLimitedBody(resp)

		if resp.StatusCode != http.StatusOK {
			return endpointError{operation: "absence-mark", statusCode: resp.StatusCode, status: resp.Status, body: string(body)}
		}

		var result []AbsenceMark
		if err := json.Unmarshal(body, &result); err != nil {
			return endpointError{operation: "absence-mark", statusCode: resp.StatusCode, status: resp.Status, body: string(body), err: err}
		}
		marks = result
		return nil
	})
	return marks, err
}

type DocumentMetadata struct {
	ID      int    `json:"ID"`
	Caption string `json:"Caption"`
	Icon    string `json:"Icon"`
}

type HomeworkSubmission struct {
	FileID     *int    `json:"FileID"`
	Text       *string `json:"Text"`
	UploadDate *string `json:"UploadDate"`
}

type fileOpenResponse struct {
	Link    string `json:"Link"`
	Caption string `json:"Caption"`
}

func (c *Client) fileBasePath() (string, error) {
	endpoint := c.endpointSnapshot()
	if endpoint.FileHash == "" {
		return "", fmt.Errorf("file endpoint hash not discovered")
	}
	wsID := extractWorkspaceID(endpoint.SchedulePath)
	if wsID == "" {
		return "", fmt.Errorf("workspace id not found in schedule path")
	}
	version := extractAPIVersion(endpoint.SchedulePath)
	return c.baseURL + version + wsID + "/" + endpoint.FileHash + "/", nil
}

func (c *Client) workspaceFileURL(path string) (string, error) {
	if strings.HasPrefix(path, "/") {
		base := strings.TrimRight(c.baseURL, "/")
		return base + path, nil
	}
	return path, nil
}

func extractAPIVersion(path string) string {
	parts := strings.Split(strings.Trim(path, "/"), "/")
	if len(parts) > 0 && strings.HasPrefix(parts[0], "v") {
		return "/" + parts[0] + "/"
	}
	return "/v0/"
}

func extractWorkspaceID(schedulePath string) string {
	parts := strings.Split(strings.Trim(schedulePath, "/"), "/")
	if len(parts) >= 2 && parts[1] != "" {
		return parts[1]
	}
	return ""
}

func (c *Client) GetDocumentMetadata(ctx context.Context, docID int) (*DocumentMetadata, error) {
	base, err := c.fileBasePath()
	if err != nil {
		return nil, err
	}

	idURL := base + "id?ID=" + strconv.Itoa(docID)
	var meta DocumentMetadata
	err = retryGet(ctx, 3, func(retryCtx context.Context) error {
		req, err := http.NewRequestWithContext(retryCtx, http.MethodGet, idURL, nil)
		if err != nil {
			return err
		}
		req.Header.Set("Accept", "application/json,*/*")
		req.Header.Set("Referer", c.baseURL+"/")
		req.Header.Set("User-Agent", "ktk-schedule/1.0")

		resp, err := c.httpClient.Do(req)
		if err != nil {
			return err
		}
		defer resp.Body.Close()

		body, _ := readLimitedBody(resp)
		if resp.StatusCode != http.StatusOK {
			return endpointError{operation: "document metadata", statusCode: resp.StatusCode, status: resp.Status, body: string(body)}
		}

		return json.Unmarshal(body, &meta)
	})
	if err != nil {
		return nil, err
	}
	return &meta, nil
}

func (c *Client) DownloadFile(ctx context.Context, link string) ([]byte, error) {
	absURL, err := c.workspaceFileURL(link)
	if err != nil {
		return nil, err
	}

	req, err := http.NewRequestWithContext(ctx, http.MethodGet, absURL, nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("Referer", c.baseURL+"/")
	req.Header.Set("User-Agent", "ktk-schedule/1.0")

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	body, _ := readDownloadBody(resp)
	if resp.StatusCode != http.StatusOK {
		return nil, endpointError{operation: "file download", statusCode: resp.StatusCode, status: resp.Status}
	}
	return body, nil
}

func readDownloadBody(resp *http.Response) ([]byte, error) {
	return io.ReadAll(io.LimitReader(resp.Body, maxDownloadBodyBytes))
}

func (c *Client) GetFileLink(ctx context.Context, docID int) (link string, caption string, err error) {
	base, err := c.fileBasePath()
	if err != nil {
		return "", "", err
	}

	openURL := base + "open?ID=" + strconv.Itoa(docID)
	var resp fileOpenResponse
	err = retryGet(ctx, 3, func(retryCtx context.Context) error {
		req, err := http.NewRequestWithContext(retryCtx, http.MethodGet, openURL, nil)
		if err != nil {
			return err
		}
		req.Header.Set("Accept", "application/json,*/*")
		req.Header.Set("Referer", c.baseURL+"/")
		req.Header.Set("User-Agent", "ktk-schedule/1.0")

		r, err := c.httpClient.Do(req)
		if err != nil {
			return err
		}
		defer r.Body.Close()

		body, _ := readLimitedBody(r)
		if r.StatusCode != http.StatusOK {
			return endpointError{operation: "file open", statusCode: r.StatusCode, status: r.Status, body: string(body)}
		}

		return json.Unmarshal(body, &resp)
	})
	if err != nil {
		return "", "", err
	}

	absLink, err := c.workspaceFileURL(resp.Link)
	if err != nil {
		return "", "", err
	}
	return absLink, resp.Caption, nil
}

func (c *Client) homeworkCheckBasePath() (string, error) {
	endpoint := c.endpointSnapshot()
	if endpoint.HomeworkHash == "" {
		return "", fmt.Errorf("homework endpoint hash not discovered")
	}
	wsID := extractWorkspaceID(endpoint.SchedulePath)
	if wsID == "" {
		return "", fmt.Errorf("workspace id not found in schedule path")
	}
	version := extractAPIVersion(endpoint.SchedulePath)
	return c.baseURL + version + wsID + "/" + endpoint.HomeworkHash + "/", nil
}

func (c *Client) GetHomeworkSubmission(ctx context.Context, sheetID int) (*HomeworkSubmission, error) {
	base, err := c.homeworkCheckBasePath()
	if err != nil {
		return nil, err
	}

	checkURL := base + "homework/check?JournalID=" + strconv.Itoa(sheetID)
	var sub HomeworkSubmission
	err = retryGet(ctx, 3, func(retryCtx context.Context) error {
		req, err := http.NewRequestWithContext(retryCtx, http.MethodGet, checkURL, nil)
		if err != nil {
			return err
		}
		req.Header.Set("Accept", "application/json,*/*")
		req.Header.Set("Referer", c.baseURL+"/")
		req.Header.Set("User-Agent", "ktk-schedule/1.0")

		r, err := c.httpClient.Do(req)
		if err != nil {
			return err
		}
		defer r.Body.Close()

		body, _ := readLimitedBody(r)
		if r.StatusCode != http.StatusOK {
			return endpointError{operation: "homework check", statusCode: r.StatusCode, status: r.Status, body: string(body)}
		}

		return json.Unmarshal(body, &sub)
	})
	if err != nil {
		return nil, err
	}
	return &sub, nil
}

func educationYear(now time.Time) int {
	year := now.Year()
	if now.Month() < time.September {
		return year - 1
	}
	return year
}

func (c *Client) buildScheduleURL(path string, groupID int, teacherHash string, weekMillis int64) (string, error) {
	requestURL, err := c.resolveURL(path)
	if err != nil {
		return "", err
	}

	parsedURL, err := url.Parse(requestURL)
	if err != nil {
		return "", err
	}

	query := parsedURL.Query()
	if teacherHash != "" {
		if c.teacherScheduleHash != "" && strings.Contains(path, c.teacherScheduleHash) {
			query.Set("Teacher", "")
			query.Set("Group", "")
		} else {
			query.Set("Teacher", teacherHash)
			query.Set("Group", "")
		}
		query.Set("Year", strconv.Itoa(educationYear(time.Now())))
	} else {
		query.Set("Teacher", "")
		query.Set("Group", strconv.Itoa(groupID))
	}
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

func logScheduleDebug(body []byte) {
	var days []struct {
		Subjects []json.RawMessage `json:"Subjects"`
	}
	if err := json.Unmarshal(body, &days); err != nil {
		slog.Warn("schedule debug decode", "error", err)
		return
	}

	var subjectCount int
	var firstSubject json.RawMessage
	var firstSubjectDay int
	var gradeCount, markCount int
	markValues := make(map[int]int)
	for dayIndex, day := range days {
		subjectCount += len(day.Subjects)
		if len(firstSubject) == 0 && len(day.Subjects) > 0 {
			firstSubject = day.Subjects[0]
			firstSubjectDay = dayIndex
		}

		var subjects []struct {
			Appraisal int `json:"Appraisal"`
			Mark      int `json:"Mark"`
		}
		allRaw, _ := json.Marshal(day.Subjects)
		json.Unmarshal(allRaw, &subjects)
		for _, s := range subjects {
			if s.Appraisal != 0 {
				gradeCount++
			}
			if s.Mark != 0 {
				markCount++
				markValues[s.Mark]++
			}
		}
	}

	slog.Debug("schedule debug", "days", len(days), "subjects", subjectCount,
		"non_zero_appraisal", gradeCount, "non_zero_mark", markCount)
	if len(markValues) > 0 {
		slog.Debug("schedule mark values", "marks", fmt.Sprintf("%v", markValues))
	}
	if len(firstSubject) == 0 {
		return
	}

	item := string(firstSubject)
	if len(item) > maxDebugScheduleItemBytes {
		item = item[:maxDebugScheduleItemBytes] + "..."
	}
	slog.Debug("schedule debug item", "day", firstSubjectDay, "item", item)
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

func WeekStart(now time.Time, loc *time.Location) time.Time {
	if loc == nil {
		loc = time.Local
	}

	t := now.In(loc)

	weekday := int(t.Weekday())
	if weekday == 0 {
		weekday = 7
	}

	return time.Date(
		t.Year(),
		t.Month(),
		t.Day(),
		6, 0, 0, 0,
		loc,
	).AddDate(0, 0, -(weekday - 1))
}

func WeekStartMillis(now time.Time, loc *time.Location) int64 {
	return WeekStart(now, loc).UnixMilli()
}

func WeekStartFromMillis(value int64, loc *time.Location) time.Time {
	if loc == nil {
		loc = time.Local
	}
	return time.UnixMilli(value).In(loc)
}

func ParseScheduleDate(raw string, now time.Time, loc *time.Location) (time.Time, error) {
	if loc == nil {
		loc = time.Local
	}

	raw = strings.TrimSpace(raw)
	if raw == "" {
		return normalizeDate(now, loc), nil
	}

	for _, layout := range []string{time.DateOnly, "02.01.2006", "2.1.2006"} {
		if parsed, err := time.ParseInLocation(layout, raw, loc); err == nil {
			return normalizeDate(parsed, loc), nil
		}
	}

	year := now.In(loc).Year()
	for _, layout := range []string{"02.01.2006", "2.1.2006"} {
		if parsed, err := time.ParseInLocation(layout, fmt.Sprintf("%s.%d", raw, year), loc); err == nil {
			return normalizeDate(parsed, loc), nil
		}
	}

	return time.Time{}, fmt.Errorf("invalid schedule date: %q", raw)
}

func WeekLabel(weekStart time.Time, loc *time.Location) string {
	if loc == nil {
		loc = time.Local
	}

	weekStart = WeekStart(weekStart, loc)
	weekEnd := weekStart.AddDate(0, 0, 5)
	return fmt.Sprintf(
		"Неделя %d (с %s по %s)",
		AcademicWeekNumber(weekStart, loc),
		formatDayMonth(weekStart),
		formatDayMonth(weekEnd),
	)
}

func AcademicWeekNumber(value time.Time, loc *time.Location) int {
	if loc == nil {
		loc = time.Local
	}

	weekStart := WeekStart(value, loc)
	start := WeekStart(time.Date(weekStart.Year(), time.September, 1, 12, 0, 0, 0, loc), loc)
	if weekStart.Before(start) {
		start = WeekStart(time.Date(weekStart.Year()-1, time.September, 1, 12, 0, 0, 0, loc), loc)
	}
	if weekStart.Before(start) {
		return 1
	}

	return int(weekStart.Sub(start).Hours()/(24*7)) + 1
}

func normalizeDate(value time.Time, loc *time.Location) time.Time {
	t := value.In(loc)
	return time.Date(t.Year(), t.Month(), t.Day(), 12, 0, 0, 0, loc)
}

func formatDayMonth(value time.Time) string {
	return fmt.Sprintf("%02d %s", value.Day(), MonthGenitive[value.Month()])
}

func FindTodayIndex(days []ScheduleDay) int {
	for i, day := range days {
		if day.Today {
			return i
		}
	}
	return 0
}

func IsSchoolDay(days []ScheduleDay, now time.Time, loc *time.Location) bool {
	if loc == nil {
		loc = time.Local
	}

	today := now.In(loc).Format(time.DateOnly)
	for _, day := range days {
		if scheduleDate(day.Date, loc) == today {
			return true
		}
	}
	return false
}

func IsNonSchoolDay(day ScheduleDay) bool {
	if len(day.Subjects) == 0 {
		return true
	}
	for _, s := range day.Subjects {
		if s.ExtendedData.PairType != 9 && s.ExtraData.LectureType != 9 {
			return false
		}
	}
	return true
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

func ParsePersonalSubgroup(value string) (string, bool) {
	switch normalizeSubgroup(value) {
	case "left":
		return "left", true
	case "right":
		return "right", true
	default:
		return "", false
	}
}

func SubgroupLabel(value string) string {
	switch normalizeSubgroup(value) {
	case "left":
		return "1 подгруппа"
	case "right":
		return "2 подгруппа"
	case "middle":
		return "общая"
	default:
		return "подгруппа не выбрана"
	}
}

func FilterScheduleDays(days []ScheduleDay, subgroup string, showAll bool) []ScheduleDay {
	if showAll {
		return days
	}

	var ok bool
	subgroup, ok = ParsePersonalSubgroup(subgroup)
	if !ok {
		return days
	}

	filtered := make([]ScheduleDay, len(days))
	for i, day := range days {
		filtered[i] = day
		filtered[i].Subjects = filterSubjects(day.Subjects, subgroup)
	}

	return filtered
}

func filterSubjects(subjects []ScheduleItem, subgroup string) []ScheduleItem {
	filtered := make([]ScheduleItem, 0, len(subjects))
	for _, subject := range subjects {
		itemSubgroup := normalizeSubgroup(subject.Subgroup)
		if _, ok := ParsePersonalSubgroup(itemSubgroup); !ok || itemSubgroup == subgroup {
			filtered = append(filtered, subject)
		}
	}
	return filtered
}

func normalizeSubgroup(value string) string {
	value = strings.ToLower(strings.TrimSpace(value))
	value = strings.ReplaceAll(value, "_", "")
	value = strings.ReplaceAll(value, "-", "")
	value = strings.ReplaceAll(value, " ", "")

	switch value {
	case "1", "left", "first", "one", "первая", "первый", "1подгруппа", "подгруппа1":
		return "left"
	case "2", "right", "second", "two", "вторая", "второй", "2подгруппа", "подгруппа2":
		return "right"
	case "", "middle", "common", "both", "all", "общая", "обе":
		return "middle"
	default:
		return value
	}
}

func FormatScheduleDay(day ScheduleDay, halls LectureHallMap) string {
	return FormatScheduleDayWithOptions(day, halls, FormatOptions{})
}

func CalculatePairTiming(preset CallPreset, pairNumber int) (PairTiming, bool) {
	idx := pairNumber - 1
	if idx < 0 || idx >= len(preset.CallSet) {
		return PairTiming{}, false
	}

	hour, min := parseBeginTime(preset.Begin)
	totalMin := hour*60 + min

	for i := range idx {
		totalMin += preset.CallSet[i].Break + preset.CallSet[i].Duration
	}

	return PairTiming{
		StartHour: totalMin / 60,
		StartMin:  totalMin % 60,
		EndHour:   (totalMin + preset.CallSet[idx].Duration) / 60,
		EndMin:    (totalMin + preset.CallSet[idx].Duration) % 60,
		Duration:  preset.CallSet[idx].Duration,
	}, true
}

func parseBeginTime(value string) (int, int) {
	parts := strings.SplitN(value, "T", 2)
	if len(parts) != 2 {
		return 0, 0
	}
	timeParts := strings.SplitN(strings.TrimSuffix(parts[1], "Z"), ":", 3)
	if len(timeParts) < 2 {
		return 0, 0
	}
	hour, _ := strconv.Atoi(timeParts[0])
	min, _ := strconv.Atoi(timeParts[1])
	return hour, min
}

func formatDuration(d time.Duration) string {
	totalMin := int(d.Minutes())
	if totalMin < 0 {
		totalMin = 0
	}
	if totalMin < 60 {
		return fmt.Sprintf("%d мин", totalMin)
	}
	hours := totalMin / 60
	mins := totalMin % 60
	if mins == 0 {
		return fmt.Sprintf("%d ч", hours)
	}
	return fmt.Sprintf("%d ч %d мин", hours, mins)
}

func FormatScheduleDayWithOptions(day ScheduleDay, halls LectureHallMap, options FormatOptions) string {
	var b strings.Builder

	label := FormatDate(day.Date)
	if day.Today {
		label = "Сегодня — " + label
	}
	b.WriteString("📅 " + label + "\n\n")

	if len(day.Subjects) == 0 && day.MaxPair == 0 {
		b.WriteString("Пар нет.")
		return b.String()
	}

	now := options.Now
	if now.IsZero() {
		now = time.Now()
	}
	if options.Loc != nil {
		now = now.In(options.Loc)
	}

	var preset CallPreset
	if options.CallPresets != nil {
		preset = options.CallPresets[day.CallPreset]
	}

	subjectsByPair := make(map[int]ScheduleItem, len(day.Subjects))
	for _, s := range day.Subjects {
		subjectsByPair[s.Pair] = s
	}

	maxPair := day.MaxPair
	if maxPair < 1 {
		maxPair = len(subjectsByPair)
	}
	if maxPair == 0 {
		b.WriteString("Пар нет.")
		return b.String()
	}

	for p := 1; p <= maxPair; p++ {
		if s, ok := subjectsByPair[p]; ok {
			writeSubjectHeader(&b, s, preset, options)
			writeTiming(&b, s, preset, day.Today, now)
			writeSubjectBody(&b, s, halls, options)
		} else {
			writeEmptyPair(&b, p, preset)
		}
	}

	return strings.TrimSpace(b.String())
}

func writeSubjectHeader(b *strings.Builder, subject ScheduleItem, preset CallPreset, options FormatOptions) {
	b.WriteString(strconv.Itoa(subject.Pair))
	b.WriteString(" пара")

	if preset.ID != 0 {
		if timing, ok := CalculatePairTiming(preset, subject.Pair); ok {
			b.WriteString(" [")
			b.WriteString(strconv.Itoa(timing.Duration))
			b.WriteString(" мин]")
		}
	}

	if options.ShowSubgroupLabels {
		if label := formatSubjectSubgroup(subject.Subgroup); label != "" {
			b.WriteString(" [")
			b.WriteString(label)
			b.WriteString("]")
		}
	}

	b.WriteString(" — ")
	b.WriteString(subject.Discipline)
	b.WriteByte('\n')
}

func writeEmptyPair(b *strings.Builder, pairNumber int, preset CallPreset) {
	b.WriteString(strconv.Itoa(pairNumber))
	b.WriteString(" пара")

	if preset.ID != 0 {
		if timing, ok := CalculatePairTiming(preset, pairNumber); ok {
			b.WriteString(" [")
			b.WriteString(strconv.Itoa(timing.Duration))
			b.WriteString(" мин]")
		}
	}

	b.WriteString(" — пусто\n")

	if preset.ID != 0 {
		if timing, ok := CalculatePairTiming(preset, pairNumber); ok {
			b.WriteString("⏰ ")
			writeTwoDigits(b, timing.StartHour)
			b.WriteByte(':')
			writeTwoDigits(b, timing.StartMin)
			b.WriteByte('-')
			writeTwoDigits(b, timing.EndHour)
			b.WriteByte(':')
			writeTwoDigits(b, timing.EndMin)
			b.WriteByte('\n')
		}
	}

	b.WriteByte('\n')
}

func writeTiming(b *strings.Builder, subject ScheduleItem, preset CallPreset, isToday bool, now time.Time) {
	if preset.ID == 0 {
		return
	}

	timing, ok := CalculatePairTiming(preset, subject.Pair)
	if !ok {
		return
	}

	b.WriteString("⏰ ")
	writeTwoDigits(b, timing.StartHour)
	b.WriteByte(':')
	writeTwoDigits(b, timing.StartMin)
	b.WriteByte('-')
	writeTwoDigits(b, timing.EndHour)
	b.WriteByte(':')
	writeTwoDigits(b, timing.EndMin)
	b.WriteByte('\n')

	if !isToday {
		return
	}

	pairStart := time.Date(now.Year(), now.Month(), now.Day(),
		timing.StartHour, timing.StartMin, 0, 0, now.Location())
	pairEnd := time.Date(now.Year(), now.Month(), now.Day(),
		timing.EndHour, timing.EndMin, 0, 0, now.Location())

	switch {
	case now.After(pairStart) && now.Before(pairEnd):
		elapsed := now.Sub(pairStart)
		remaining := pairEnd.Sub(now)
		b.WriteString("⏳ идёт ")
		b.WriteString(formatDuration(elapsed))
		b.WriteString(", осталось ")
		b.WriteString(formatDuration(remaining))
		b.WriteByte('\n')
	case now.Before(pairStart) && pairStart.Sub(now) <= time.Hour:
		b.WriteString("⏳ начнётся через ")
		b.WriteString(formatDuration(pairStart.Sub(now)))
		b.WriteByte('\n')
	}
}

func writeSubjectBody(b *strings.Builder, subject ScheduleItem, halls LectureHallMap, options FormatOptions) {
	if label := pairTypeName(subject.ExtraData.LectureType, options.PairTypes); label != "" {
		b.WriteString(label)
		b.WriteByte('\n')
	} else if label := pairTypeName(subject.ExtendedData.PairType, options.PairTypes); label != "" {
		b.WriteString(label)
		b.WriteByte('\n')
	}

	if options.IsTeacher {
		if subject.Group != "" {
			b.WriteString("👥 Группа: " + subject.Group + "\n")
		}
	} else {
		writeAppraisal(b, subject)

		if !MarkIsGradeModifier(subject.Mark) && subject.Mark != 0 {
			b.WriteString("📊 Отметка: ")
			b.WriteString(FormatMarkWithMap(subject.Mark, options.AbsenceByDigit))
			b.WriteByte('\n')
		}

		if subject.Teacher != "" {
			b.WriteString("👤 " + subject.Teacher + "\n")
		}

		if subject.Group != "" {
			b.WriteString("👥 Группа: " + subject.Group + "\n")
		}
	}

	b.WriteString("🏫 Кабинет: " + FormatLectureHall(subject.LectureHall, halls) + "\n")

	if subject.ExtraData.Homework.Task != nil && strings.TrimSpace(*subject.ExtraData.Homework.Task) != "" {
		b.WriteString("Задание: " + strings.TrimSpace(*subject.ExtraData.Homework.Task) + "\n")
	}

	if subject.ExtraData.Homework.Webinar != nil && strings.TrimSpace(*subject.ExtraData.Homework.Webinar) != "" {
		b.WriteString("Вебинар: " + strings.TrimSpace(*subject.ExtraData.Homework.Webinar) + "\n")
	}

	if n := len(subject.ExtraData.Homework.Files); n > 0 {
		b.WriteString("📎 ")
		b.WriteString(strconv.Itoa(n))
		switch {
		case n%10 == 1 && n%100 != 11:
			b.WriteString(" файл")
		case n%10 >= 2 && n%10 <= 4 && (n%100 < 10 || n%100 >= 20):
			b.WriteString(" файла")
		default:
			b.WriteString(" файлов")
		}

		showNames := n <= 20
		for _, id := range subject.ExtraData.Homework.Files {
			if name, ok := options.FileNames[id]; ok && showNames {
				b.WriteString("\n  \u2022 ")
				b.WriteString(name)
			}
		}
		b.WriteByte('\n')
	}

	if !options.IsTeacher {
		if studentFileID, ok := options.StudentFiles[subject.ExtraData.Sheet]; ok {
			b.WriteString("📤 1 файл (моя работа)\n")
			if name, ok := options.FileNames[studentFileID]; ok {
				b.WriteString("  \u2022 ")
				b.WriteString(name)
				b.WriteByte('\n')
			}
		}
	}

	b.WriteByte('\n')
}

func writeAppraisal(b *strings.Builder, subject ScheduleItem) {
	m := subject.Mark
	hasAppraisal := subject.Appraisal != 0
	isMod := MarkIsGradeModifier(m)

	if !hasAppraisal && !isMod {
		return
	}

	b.WriteString("📊 Оценка: ")
	if hasAppraisal {
		b.WriteString(strconv.Itoa(subject.Appraisal))
	}
	switch m {
	case markGradePlus:
		b.WriteByte('+')
	case markGradeMinus:
		b.WriteByte('-')
	}
	b.WriteByte('\n')
}

func writeTwoDigits(b *strings.Builder, n int) {
	if n < 10 {
		b.WriteByte('0')
	}
	b.WriteString(strconv.Itoa(n))
}

func formatSubjectSubgroup(value string) string {
	switch normalizeSubgroup(value) {
	case "left":
		return "1"
	case "right":
		return "2"
	default:
		return ""
	}
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

func retryGet(ctx context.Context, maxAttempts int, fn func(context.Context) error) error {
	var lastErr error
	for attempt := range maxAttempts {
		if attempt > 0 {
			slog.Debug("retry", "attempt", attempt, "max_attempts", maxAttempts, "error", lastErr)
			select {
			case <-ctx.Done():
				return ctx.Err()
			case <-time.After(backoff(attempt)):
			}
		}
		if err := fn(ctx); err != nil {
			lastErr = err
			if !isTransient(err) {
				return err
			}
			continue
		}
		return nil
	}
	return lastErr
}

func backoff(attempt int) time.Duration {
	return time.Duration(50<<min(attempt-1, 5)) * time.Millisecond
}

func isTransient(err error) bool {
	if err == nil {
		return false
	}
	var urlErr *url.Error
	if errors.As(err, &urlErr) {
		return true
	}
	var epErr endpointError
	if errors.As(err, &epErr) && epErr.statusCode >= 500 {
		return true
	}
	return false
}

func FormatMark(value int, absenceMarks []AbsenceMark) string {
	m := make(map[int]string, len(absenceMarks))
	for _, am := range absenceMarks {
		m[am.Digit] = am.Caption
	}
	return FormatMarkWithMap(value, m)
}

func FormatMarkWithMap(value int, absenceByDigit map[int]string) string {
	if symbol, ok := markSymbols[value]; ok {
		return symbol
	}

	if reason, ok := absenceByDigit[value]; ok {
		if sickMarkDigits[value] {
			return "Б " + reason
		}
		return "Н " + reason
	}

	return strconv.Itoa(value)
}

func MarkIsGradeModifier(value int) bool {
	return value == markGradePlus || value == markGradeMinus
}
