package ktk

import "strings"

type Endpoints struct {
	SignInPath      string
	SchedulePath    string
	LectureHallPath string
	CallPresetPath  string
	AbsenceMarkPath string
	PairTypePath    string
	BranchID        string
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
	if strings.TrimSpace(e.SchedulePath) == "" {
		e.SchedulePath = fallback.SchedulePath
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

	e.SignInPath = normalizeEndpointPath(e.SignInPath)
	e.SchedulePath = normalizeEndpointPath(e.SchedulePath)
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
