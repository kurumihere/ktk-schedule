package ktk

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"log/slog"
	"net/http"
	"net/url"
	"path"
	"regexp"
	"strings"
)

const maxDiscoveredScripts = 64

var (
	scriptSrcPattern = regexp.MustCompile(`(?is)<script[^>]+src=["']([^"']+)["']`)
	jsPathPattern    = regexp.MustCompile(`["']([^"']+\.js(?:\?[^"']*)?)["']`)
	apiPathPattern   = regexp.MustCompile(`/v[0-9]+/[A-Za-z0-9_-]+/[A-Za-z0-9_-]+/(?:lecture-hall|[A-Za-z0-9_-]+)`)
	branchPatterns   = []*regexp.Regexp{
		regexp.MustCompile(`Branch(?:%3[Dd]|=)([A-Za-z0-9_-]+)`),
		regexp.MustCompile(`Branch["']?\s*[:=]\s*["']([A-Za-z0-9_-]+)["']`),
	}
)

type endpointCandidates struct {
	schedulePaths    []string
	lectureHallPaths []string
	branchIDs        []string
}

var fallbackScheduleHashes = []string{
	"f88efc44efafbd74",
}

func (c *Client) RefreshEndpoints(ctx context.Context, groupID int, weekMillis int64) error {
	candidates, err := c.discoverEndpointCandidates(ctx)
	if err != nil {
		return err
	}

	next := c.endpointSnapshot()

	bestPath, foundGrades, _ := c.pickScheduleEndpoint(ctx, candidates.schedulePaths, groupID, weekMillis)
	if bestPath == "" {
		fallbackBase := next.CallPresetPath
		if fallbackBase == "" && len(candidates.schedulePaths) > 0 {
			fallbackBase = candidates.schedulePaths[0]
		}
		if base := trimLastSegment(fallbackBase); base != "" {
			for _, hash := range fallbackScheduleHashes {
				fallbackPath := path.Join(base, hash)
				if fp, fg, _ := c.pickScheduleEndpoint(ctx, []string{fallbackPath}, groupID, weekMillis); fp != "" {
					bestPath = fp
					foundGrades = fg
					slog.Debug("fallback schedule endpoint", "path", fp, "has_grades", fg)
					break
				}
			}
		}
	}

	if bestPath == "" {
		return fmt.Errorf("schedule endpoint not found")
	}

	next.SchedulePath = bestPath
	if !foundGrades {
		slog.Warn("no schedule endpoint with grades found, grades will be unavailable")
	}

	branchID := next.BranchID
	if len(candidates.branchIDs) > 0 {
		branchID = candidates.branchIDs[0]
	}
	for _, p := range candidates.lectureHallPaths {
		if err := c.validateLectureHallEndpoint(ctx, p, branchID); err == nil {
			next.LectureHallPath = p
			next.BranchID = branchID
			break
		}
	}

	if next.CallPresetPath == "" {
		next.CallPresetPath = DeriveCallPresetPath(next.SchedulePath)
	}
	if next.AbsenceMarkPath == "" {
		next.AbsenceMarkPath = DeriveAbsenceMarkPath(next.SchedulePath)
	}

	c.setEndpoints(next)
	slog.Debug("endpoints refreshed",
		"schedule_path", next.SchedulePath,
		"lecture_hall_path", next.LectureHallPath,
		"call_preset_path", next.CallPresetPath,
		"branch_id", next.BranchID,
	)
	return nil
}

func (c *Client) pickScheduleEndpoint(ctx context.Context, paths []string, groupID int, weekMillis int64) (bestPath string, hasGrades bool, bestCount int) {
	bestCount = -1
	for _, p := range paths {
		days, raw, err := c.validateScheduleEndpoint(ctx, p, groupID, weekMillis)
		if err != nil {
			continue
		}
		pathHasGrades := scheduleHasGrades(raw)
		if !pathHasGrades && hasGrades {
			continue
		}
		subjectCount := countSubjects(days)
		if pathHasGrades || subjectCount > bestCount {
			bestPath = p
			bestCount = subjectCount
			hasGrades = hasGrades || pathHasGrades
		}
	}
	return
}

func (c *Client) discoverEndpointCandidates(ctx context.Context) (endpointCandidates, error) {
	rootURL, err := c.resolveURL("/")
	if err != nil {
		return endpointCandidates{}, err
	}

	body, err := c.fetchText(ctx, rootURL)
	if err != nil {
		return endpointCandidates{}, err
	}

	candidates := endpointCandidates{}
	candidates.addFromText(body)

	queue := extractScriptURLs(body, rootURL)
	visited := make(map[string]bool)

	for len(queue) > 0 && len(visited) < maxDiscoveredScripts {
		scriptURL := queue[0]
		queue = queue[1:]

		if visited[scriptURL] || !sameOrigin(c.baseURL, scriptURL) {
			continue
		}
		visited[scriptURL] = true

		text, err := c.fetchText(ctx, scriptURL)
		if err != nil {
			continue
		}

		candidates.addFromText(text)
		queue = append(queue, extractScriptURLs(text, scriptURL)...)
	}

	return candidates, nil
}

func (c *Client) fetchText(ctx context.Context, requestURL string) (string, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, requestURL, nil)
	if err != nil {
		return "", err
	}
	req.Header.Set("Accept", "text/html,application/javascript,*/*")
	req.Header.Set("Referer", c.baseURL+"/")
	req.Header.Set("User-Agent", "ktk-schedule/1.0")

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()

	body, _ := readLimitedBody(resp)
	if resp.StatusCode != http.StatusOK {
		return "", endpointError{operation: "endpoint discovery", statusCode: resp.StatusCode, status: resp.Status, body: string(body)}
	}

	return string(body), nil
}

func (c *Client) validateScheduleEndpoint(ctx context.Context, path string, groupID int, weekMillis int64) ([]ScheduleDay, []byte, error) {
	requestURL, err := c.buildScheduleURL(path, groupID, weekMillis)
	if err != nil {
		return nil, nil, err
	}

	body, statusCode, status, err := c.getJSON(ctx, requestURL)
	if err != nil {
		return nil, nil, err
	}
	if statusCode != http.StatusOK {
		return nil, nil, endpointError{operation: "schedule endpoint validation", statusCode: statusCode, status: status, body: string(body)}
	}

	var days []ScheduleDay
	if err := json.Unmarshal(body, &days); err != nil {
		return nil, nil, fmt.Errorf("validate schedule endpoint: %w", err)
	}
	if len(days) == 0 {
		return nil, nil, fmt.Errorf("validate schedule endpoint: empty schedule")
	}

	return days, body, nil
}

func (c *Client) validateLectureHallEndpoint(ctx context.Context, path, branchID string) error {
	requestURL, err := c.buildLectureHallURL(path, branchID)
	if err != nil {
		return err
	}

	body, statusCode, status, err := c.getJSON(ctx, requestURL)
	if err != nil {
		return err
	}
	if statusCode != http.StatusOK {
		return endpointError{operation: "lecture hall endpoint validation", statusCode: statusCode, status: status, body: string(body)}
	}

	var data lectureHallResponse
	if err := json.Unmarshal(body, &data); err != nil {
		return fmt.Errorf("validate lecture hall endpoint: %w", err)
	}

	return nil
}

func (c *Client) getJSON(ctx context.Context, requestURL string) ([]byte, int, string, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, requestURL, nil)
	if err != nil {
		return nil, 0, "", err
	}
	req.Header.Set("Accept", "application/json,*/*")
	req.Header.Set("Referer", c.baseURL+"/")
	req.Header.Set("User-Agent", "ktk-schedule/1.0")

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return nil, 0, "", err
	}
	defer resp.Body.Close()

	body, _ := readLimitedBody(resp)
	return body, resp.StatusCode, resp.Status, nil
}

func (c *endpointCandidates) addFromText(text string) {
	for _, match := range apiPathPattern.FindAllString(text, -1) {
		if strings.Contains(match, "/lecture-hall") {
			c.lectureHallPaths = appendUnique(c.lectureHallPaths, match)
			continue
		}
		c.schedulePaths = appendUnique(c.schedulePaths, match)
	}

	for _, pattern := range branchPatterns {
		for _, match := range pattern.FindAllStringSubmatch(text, -1) {
			if len(match) == 2 {
				c.branchIDs = appendUnique(c.branchIDs, match[1])
			}
		}
	}
}

func countSubjects(days []ScheduleDay) int {
	var count int
	for _, day := range days {
		count += len(day.Subjects)
	}
	return count
}

func scheduleHasGrades(raw []byte) bool {
	return bytes.Contains(raw, []byte(`"Appraisal"`)) || bytes.Contains(raw, []byte(`"Mark"`))
}

func trimLastSegment(p string) string {
	p = strings.TrimRight(p, "/")
	i := strings.LastIndex(p, "/")
	if i < 0 {
		return ""
	}
	return p[:i]
}

func extractScriptURLs(text, base string) []string {
	var result []string

	for _, match := range scriptSrcPattern.FindAllStringSubmatch(text, -1) {
		if len(match) == 2 {
			result = appendUnique(result, resolveReference(base, match[1]))
		}
	}
	for _, match := range jsPathPattern.FindAllStringSubmatch(text, -1) {
		if len(match) == 2 {
			result = appendUnique(result, resolveReference(base, match[1]))
		}
	}

	return result
}

func resolveReference(base, reference string) string {
	baseURL, err := url.Parse(base)
	if err != nil {
		return reference
	}
	referenceURL, err := url.Parse(reference)
	if err != nil {
		return reference
	}
	return baseURL.ResolveReference(referenceURL).String()
}

func sameOrigin(base, target string) bool {
	baseURL, err := url.Parse(base)
	if err != nil {
		return false
	}
	targetURL, err := url.Parse(target)
	if err != nil {
		return false
	}
	return baseURL.Scheme == targetURL.Scheme && baseURL.Host == targetURL.Host
}

func appendUnique(values []string, value string) []string {
	value = strings.TrimSpace(value)
	if value == "" {
		return values
	}
	for _, existing := range values {
		if existing == value {
			return values
		}
	}
	return append(values, value)
}
