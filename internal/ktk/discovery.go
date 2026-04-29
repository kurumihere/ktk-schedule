package ktk

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/url"
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

func (c *Client) RefreshEndpoints(ctx context.Context, groupID int, weekMillis int64) error {
	candidates, err := c.discoverEndpointCandidates(ctx)
	if err != nil {
		return err
	}

	current := c.endpointSnapshot()
	next := current

	bestSubjectCount := -1
	for _, path := range candidates.schedulePaths {
		days, err := c.validateScheduleEndpoint(ctx, path, groupID, weekMillis)
		if err != nil {
			continue
		}

		subjectCount := countSubjects(days)
		if subjectCount > bestSubjectCount {
			bestSubjectCount = subjectCount
			next.SchedulePath = path
		}
	}
	if next.SchedulePath == "" {
		return fmt.Errorf("schedule endpoint not found")
	}

	branchID := current.BranchID
	if len(candidates.branchIDs) > 0 {
		branchID = candidates.branchIDs[0]
	}

	for _, path := range candidates.lectureHallPaths {
		if err := c.validateLectureHallEndpoint(ctx, path, branchID); err == nil {
			next.LectureHallPath = path
			next.BranchID = branchID
			break
		}
	}

	c.setEndpoints(next)
	return nil
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

func (c *Client) validateScheduleEndpoint(ctx context.Context, path string, groupID int, weekMillis int64) ([]ScheduleDay, error) {
	requestURL, err := c.buildScheduleURL(path, groupID, weekMillis)
	if err != nil {
		return nil, err
	}

	body, statusCode, status, err := c.getJSON(ctx, requestURL)
	if err != nil {
		return nil, err
	}
	if statusCode != http.StatusOK {
		return nil, endpointError{operation: "schedule endpoint validation", statusCode: statusCode, status: status, body: string(body)}
	}

	var days []ScheduleDay
	if err := json.Unmarshal(body, &days); err != nil {
		return nil, fmt.Errorf("validate schedule endpoint: %w", err)
	}
	if len(days) == 0 {
		return nil, fmt.Errorf("validate schedule endpoint: empty schedule")
	}

	return days, nil
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
