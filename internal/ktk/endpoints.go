package ktk

import "strings"

type Endpoints struct {
	SignInPath      string
	SchedulePath    string
	LectureHallPath string
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
	if strings.TrimSpace(e.BranchID) == "" {
		e.BranchID = fallback.BranchID
	}

	e.SignInPath = normalizeEndpointPath(e.SignInPath)
	e.SchedulePath = normalizeEndpointPath(e.SchedulePath)
	e.LectureHallPath = normalizeEndpointPath(e.LectureHallPath)
	e.BranchID = strings.TrimSpace(e.BranchID)
	return e
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
