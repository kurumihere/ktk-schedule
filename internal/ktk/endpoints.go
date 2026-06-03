package ktk

import (
	"net/url"
	"strings"
)

type Endpoints struct {
	SignInPath        string
	InfoPath          string
	SchedulePath      string
	GroupSchedulePath string
	LectureHallPath   string
	CallPresetPath    string
	AbsenceMarkPath   string
	PairTypePath      string
	FileHash          string
	HomeworkHash      string
	BranchID          string
}

func DefaultEndpoints() Endpoints {
	return Endpoints{
		SignInPath: "/sign-in",
	}
}

func (e Endpoints) WithFallback(fallback Endpoints) Endpoints {
	if strings.TrimSpace(e.SignInPath) == "" {
		e.SignInPath = fallback.SignInPath
	}
	if strings.TrimSpace(e.InfoPath) == "" {
		e.InfoPath = fallback.InfoPath
	}
	if strings.TrimSpace(e.SchedulePath) == "" {
		e.SchedulePath = fallback.SchedulePath
	}
	if strings.TrimSpace(e.GroupSchedulePath) == "" {
		e.GroupSchedulePath = fallback.GroupSchedulePath
	}
	if strings.TrimSpace(e.LectureHallPath) == "" {
		e.LectureHallPath = fallback.LectureHallPath
	}
	if strings.TrimSpace(e.CallPresetPath) == "" {
		e.CallPresetPath = fallback.CallPresetPath
	}
	if strings.TrimSpace(e.AbsenceMarkPath) == "" {
		e.AbsenceMarkPath = fallback.AbsenceMarkPath
	}
	if strings.TrimSpace(e.PairTypePath) == "" {
		e.PairTypePath = fallback.PairTypePath
	}
	if strings.TrimSpace(e.BranchID) == "" {
		e.BranchID = fallback.BranchID
	}
	if strings.TrimSpace(e.FileHash) == "" {
		e.FileHash = fallback.FileHash
	}
	if strings.TrimSpace(e.HomeworkHash) == "" {
		e.HomeworkHash = fallback.HomeworkHash
	}

	e.SignInPath = normalizeEndpointPath(e.SignInPath)
	e.InfoPath = normalizeEndpointPath(e.InfoPath)
	e.SchedulePath = normalizeEndpointPath(e.SchedulePath)
	e.GroupSchedulePath = normalizeEndpointPath(e.GroupSchedulePath)
	e.LectureHallPath = normalizeEndpointPath(e.LectureHallPath)
	e.CallPresetPath = normalizeEndpointPath(e.CallPresetPath)
	e.AbsenceMarkPath = normalizeEndpointPath(e.AbsenceMarkPath)
	e.PairTypePath = normalizeEndpointPath(e.PairTypePath)
	e.BranchID = strings.TrimSpace(e.BranchID)
	return e
}

func DeriveCallPresetPath(schedulePath string) string {
	if strings.TrimSpace(schedulePath) == "" {
		return ""
	}
	i := strings.LastIndex(strings.TrimRight(schedulePath, "/"), "/")
	if i < 0 {
		return ""
	}
	return schedulePath[:i] + "/call-preset"
}

func DeriveInfoPath(schedulePath string) string {
	if strings.TrimSpace(schedulePath) == "" {
		return ""
	}
	parsedURL, err := url.Parse(schedulePath)
	if err == nil && parsedURL.Path != "" {
		schedulePath = parsedURL.Path
	}
	i := strings.LastIndex(strings.TrimRight(schedulePath, "/"), "/")
	if i < 0 {
		return ""
	}
	return schedulePath[:i] + "/info"
}

func DeriveAbsenceMarkPath(schedulePath string) string {
	if strings.TrimSpace(schedulePath) == "" {
		return ""
	}
	parts := strings.Split(strings.Trim(schedulePath, "/"), "/")
	if len(parts) < 2 {
		return ""
	}
	return "/" + parts[0] + "/" + parts[1] + "/absence/mark"
}

func normalizeEndpointPath(path string) string {
	path = strings.TrimSpace(path)
	if path == "" || strings.HasPrefix(path, "http://") || strings.HasPrefix(path, "https://") {
		return path
	}
	if !strings.HasPrefix(path, "/") {
		return "/" + path
	}
	return path
}
