package ktk

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"log/slog"
	"net/http"
	"net/url"
	"regexp"
	"strconv"
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
	fileHashPattern     = regexp.MustCompile(`/v[0-9]+/[A-Za-z0-9_-]+/([A-Za-z0-9_-]+)/(?:user-file|id|open|can-view)(?:[?"'/\s]|$)`)
	homeworkHashPattern = regexp.MustCompile(`/v[0-9]+/[A-Za-z0-9_-]+/([A-Za-z0-9_-]+)/homework/check`)
)

type endpointCandidates struct {
	infoPaths        []string
	schedulePaths    []string
	lectureHallPaths []string
	callPresetPaths  []string
	pairTypePaths    []string
	fileHashes       []string
	homeworkHashes   []string
	branchIDs        []string

	infoPathSet        map[string]struct{}
	schedulePathSet    map[string]struct{}
	lectureHallPathSet map[string]struct{}
	callPresetPathSet  map[string]struct{}
	pairTypePathSet    map[string]struct{}
	fileHashSet        map[string]struct{}
	homeworkHashSet    map[string]struct{}
	branchIDSet        map[string]struct{}
}

func (c *Client) RefreshEndpoints(ctx context.Context, groupID int, weekMillis int64, teacherHash ...string) error {
	candidates, err := c.discoverEndpointCandidates(ctx)
	if err != nil {
		return err
	}

	tHash := ""
	if len(teacherHash) > 0 {
		tHash = teacherHash[0]
	}
	isTeacher := tHash != ""

	next := c.endpointSnapshot()

	if next.InfoPath == "" && len(candidates.infoPaths) > 0 {
		next.InfoPath = candidates.infoPaths[0]
	}

	bestPath, groupPath, foundGrades := c.pickScheduleEndpoints(ctx, candidates.schedulePaths, groupID, tHash, weekMillis, !isTeacher)

	if bestPath == "" {
		return fmt.Errorf("schedule endpoint not found")
	}

	next.SchedulePath = bestPath
	if groupPath != "" {
		next.GroupSchedulePath = groupPath
	}
	if next.InfoPath == "" {
		next.InfoPath = DeriveInfoPath(next.SchedulePath)
	}
	if !isTeacher && !foundGrades {
		slog.Warn("no schedule endpoint with grades found, grades will be unavailable")
	}

	c.resolveSecondaryEndpoints(ctx, &next, candidates)

	c.setEndpoints(next)
	c.logEndpoints(next)
	return nil
}

func (c *Client) resolveSecondaryEndpoints(ctx context.Context, next *Endpoints, candidates endpointCandidates) {
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

	for _, p := range candidates.callPresetPaths {
		if err := c.validateCallPresetEndpoint(ctx, p); err == nil {
			next.CallPresetPath = p
			break
		}
	}
	if next.CallPresetPath == "" {
		next.CallPresetPath = DeriveCallPresetPath(next.SchedulePath)
	}
	if next.AbsenceMarkPath == "" {
		next.AbsenceMarkPath = DeriveAbsenceMarkPath(next.SchedulePath)
	}

	for _, p := range candidates.pairTypePaths {
		if err := c.validatePairTypeEndpoint(ctx, p); err == nil {
			next.PairTypePath = p
			break
		}
	}

	if len(candidates.fileHashes) > 0 {
		next.FileHash = candidates.fileHashes[0]
	}
	if len(candidates.homeworkHashes) > 0 {
		next.HomeworkHash = candidates.homeworkHashes[0]
	}
}

func (c *Client) logEndpoints(next Endpoints) {
	slog.Debug("endpoints refreshed",
		"schedule_path", next.SchedulePath,
		"group_schedule_path", next.GroupSchedulePath,
		"info_path", next.InfoPath,
		"lecture_hall_path", next.LectureHallPath,
		"call_preset_path", next.CallPresetPath,
		"absence_mark_path", next.AbsenceMarkPath,
		"pair_type_path", next.PairTypePath,
		"file_hash", next.FileHash,
		"homework_hash", next.HomeworkHash,
		"branch_id", next.BranchID,
	)
}

func (c *Client) refreshAuxiliaryEndpoints(ctx context.Context) error {
	candidates, err := c.discoverEndpointCandidates(ctx)
	if err != nil {
		return err
	}

	next := c.endpointSnapshot()
	changed := false
	if len(candidates.fileHashes) > 0 && next.FileHash != candidates.fileHashes[0] {
		next.FileHash = candidates.fileHashes[0]
		changed = true
	}
	if len(candidates.homeworkHashes) > 0 && next.HomeworkHash != candidates.homeworkHashes[0] {
		next.HomeworkHash = candidates.homeworkHashes[0]
		changed = true
	}
	if !changed {
		return fmt.Errorf("auxiliary endpoints not found")
	}

	c.setEndpoints(next)
	slog.Debug("auxiliary endpoints refreshed", "file_hash", next.FileHash, "homework_hash", next.HomeworkHash)
	return nil
}

func (c *Client) pickScheduleEndpoint(ctx context.Context, paths []string, groupID int, teacherHash string, weekMillis int64, preferGrades bool) (bestPath string, hasGrades bool, bestCount int) {
	bestCount = -1
	for _, p := range paths {
		days, raw, err := c.validateScheduleEndpoint(ctx, p, groupID, teacherHash, weekMillis)
		if err != nil {
			continue
		}
		pathHasGrades := scheduleHasGrades(raw)
		if preferGrades && !pathHasGrades && hasGrades {
			continue
		}
		subjectCount := countSubjects(days)
		if (preferGrades && pathHasGrades && !hasGrades) || subjectCount > bestCount {
			bestPath = p
			bestCount = subjectCount
			hasGrades = hasGrades || pathHasGrades
		}
	}
	return
}

func (c *Client) pickScheduleEndpoints(ctx context.Context, paths []string, groupID int, teacherHash string, weekMillis int64, preferGrades bool) (bestPath string, groupPath string, hasGrades bool) {
	if teacherHash != "" {
		bestPath, hasGrades, _ = c.pickScheduleEndpoint(ctx, paths, groupID, teacherHash, weekMillis, preferGrades)
		return bestPath, "", hasGrades
	}

	type candidate struct {
		path       string
		days       []ScheduleDay
		raw        []byte
		count      int
		hasGrades  bool
		groupAware bool
	}

	candidates := make([]candidate, 0, len(paths))
	for _, p := range paths {
		days, raw, err := c.validateScheduleEndpoint(ctx, p, groupID, "", weekMillis)
		if err != nil {
			continue
		}

		item := candidate{
			path:      p,
			days:      days,
			raw:       raw,
			count:     countSubjects(days),
			hasGrades: scheduleHasGrades(raw),
		}
		item.groupAware = c.schedulePathVariesByGroup(ctx, p, groupID, weekMillis, item.days)
		candidates = append(candidates, item)
	}

	bestPersonal := -1
	bestGroup := -1
	for i, item := range candidates {
		if item.groupAware {
			if bestGroup < 0 || item.count > candidates[bestGroup].count {
				bestGroup = i
			}
			continue
		}

		if bestPersonal < 0 ||
			(preferGrades && item.hasGrades && !candidates[bestPersonal].hasGrades) ||
			(item.hasGrades == candidates[bestPersonal].hasGrades && item.count > candidates[bestPersonal].count) {
			bestPersonal = i
		}
	}

	if bestGroup >= 0 {
		groupPath = candidates[bestGroup].path
	}
	if bestPersonal >= 0 {
		bestPath = candidates[bestPersonal].path
		hasGrades = candidates[bestPersonal].hasGrades
		return bestPath, groupPath, hasGrades
	}
	if bestGroup >= 0 {
		bestPath = candidates[bestGroup].path
		hasGrades = candidates[bestGroup].hasGrades
	}
	return bestPath, groupPath, hasGrades
}

func (c *Client) schedulePathVariesByGroup(ctx context.Context, path string, groupID int, weekMillis int64, baseDays []ScheduleDay) bool {
	for _, alternateGroupID := range []int{groupID - 1, groupID + 1} {
		if alternateGroupID <= 0 || alternateGroupID == groupID {
			continue
		}
		alternateDays, _, err := c.validateScheduleEndpoint(ctx, path, alternateGroupID, "", weekMillis)
		if err != nil || countSubjects(alternateDays) == 0 {
			continue
		}
		if scheduleFingerprint(alternateDays) != scheduleFingerprint(baseDays) {
			return true
		}
	}
	return false
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
	defer func() { _ = resp.Body.Close() }()

	body, _ := readLimitedBody(resp)
	if resp.StatusCode != http.StatusOK {
		return "", endpointError{operation: "endpoint discovery", statusCode: resp.StatusCode, status: resp.Status, body: string(body)}
	}

	return string(body), nil
}

func (c *Client) validateScheduleEndpoint(ctx context.Context, path string, groupID int, teacherHash string, weekMillis int64) ([]ScheduleDay, []byte, error) {
	requestURL, err := c.buildScheduleURL(path, groupID, teacherHash, weekMillis)
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

	days, err := parseScheduleDays(body)
	if err != nil {
		return nil, nil, fmt.Errorf("validate schedule endpoint: %w", err)
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

func (c *Client) validatePairTypeEndpoint(ctx context.Context, path string) error {
	requestURL, err := c.resolveURL(path)
	if err != nil {
		return err
	}

	body, statusCode, status, err := c.getJSON(ctx, requestURL)
	if err != nil {
		return err
	}
	if statusCode != http.StatusOK {
		return endpointError{operation: "pair-type endpoint validation", statusCode: statusCode, status: status, body: string(body)}
	}

	var types []PairType
	if err := json.Unmarshal(body, &types); err != nil {
		return fmt.Errorf("validate pair-type endpoint: %w", err)
	}
	if len(types) == 0 {
		return fmt.Errorf("validate pair-type endpoint: empty types")
	}

	return nil
}

func (c *Client) validateCallPresetEndpoint(ctx context.Context, path string) error {
	requestURL, err := c.resolveURL(path)
	if err != nil {
		return err
	}

	body, statusCode, status, err := c.getJSON(ctx, requestURL)
	if err != nil {
		return err
	}
	if statusCode != http.StatusOK {
		return endpointError{operation: "call-preset endpoint validation", statusCode: statusCode, status: status, body: string(body)}
	}

	var presets []CallPreset
	if err := json.Unmarshal(body, &presets); err != nil {
		return fmt.Errorf("validate call-preset endpoint: %w", err)
	}
	if len(presets) == 0 {
		return fmt.Errorf("validate call-preset endpoint: empty presets")
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
	defer func() { _ = resp.Body.Close() }()

	body, _ := readLimitedBody(resp)
	return body, resp.StatusCode, resp.Status, nil
}

func (c *endpointCandidates) addFromText(text string) {
	for _, match := range apiPathPattern.FindAllString(text, -1) {
		if strings.HasSuffix(match, "/info") {
			c.infoPaths, c.infoPathSet = appendUniqueSeen(c.infoPaths, c.infoPathSet, match)
			continue
		}
		if strings.Contains(match, "/lecture-hall") {
			c.lectureHallPaths, c.lectureHallPathSet = appendUniqueSeen(c.lectureHallPaths, c.lectureHallPathSet, match)
			continue
		}
		if strings.Contains(match, "/call-preset") {
			c.callPresetPaths, c.callPresetPathSet = appendUniqueSeen(c.callPresetPaths, c.callPresetPathSet, match)
			continue
		}
		if strings.Contains(match, "/pair-type") {
			c.pairTypePaths, c.pairTypePathSet = appendUniqueSeen(c.pairTypePaths, c.pairTypePathSet, match)
			continue
		}
		c.schedulePaths, c.schedulePathSet = appendUniqueSeen(c.schedulePaths, c.schedulePathSet, match)
	}

	for _, match := range fileHashPattern.FindAllStringSubmatch(text, -1) {
		if len(match) == 2 {
			c.fileHashes, c.fileHashSet = appendUniqueSeen(c.fileHashes, c.fileHashSet, match[1])
		}
	}

	for _, match := range homeworkHashPattern.FindAllStringSubmatch(text, -1) {
		if len(match) == 2 {
			c.homeworkHashes, c.homeworkHashSet = appendUniqueSeen(c.homeworkHashes, c.homeworkHashSet, match[1])
		}
	}

	for _, pattern := range branchPatterns {
		for _, match := range pattern.FindAllStringSubmatch(text, -1) {
			if len(match) == 2 {
				c.branchIDs, c.branchIDSet = appendUniqueSeen(c.branchIDs, c.branchIDSet, match[1])
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

func scheduleFingerprint(days []ScheduleDay) string {
	var b strings.Builder
	for _, day := range days {
		b.WriteString(day.Date)
		b.WriteByte('|')
		for _, subject := range day.Subjects {
			b.WriteString(strconv.Itoa(subject.Pair))
			b.WriteByte(':')
			b.WriteString(subject.Discipline)
			b.WriteByte(':')
			b.WriteString(subject.Teacher)
			b.WriteByte(':')
			b.WriteString(strconv.Itoa(subject.LectureHall))
			b.WriteByte(':')
			b.WriteString(subject.Subgroup)
			b.WriteByte(';')
		}
		b.WriteByte('\n')
	}
	return b.String()
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
	var seen map[string]struct{}

	for _, match := range scriptSrcPattern.FindAllStringSubmatch(text, -1) {
		if len(match) == 2 {
			result, seen = appendUniqueSeen(result, seen, resolveReference(base, match[1]))
		}
	}
	for _, match := range jsPathPattern.FindAllStringSubmatch(text, -1) {
		if len(match) == 2 {
			result, seen = appendUniqueSeen(result, seen, resolveReference(base, match[1]))
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

func appendUniqueSeen(values []string, seen map[string]struct{}, value string) ([]string, map[string]struct{}) {
	value = strings.TrimSpace(value)
	if value == "" {
		return values, seen
	}

	if seen == nil {
		seen = make(map[string]struct{}, len(values)+1)
		for _, existing := range values {
			seen[existing] = struct{}{}
		}
	}
	if _, ok := seen[value]; ok {
		return values, seen
	}

	seen[value] = struct{}{}
	return append(values, value), seen
}
