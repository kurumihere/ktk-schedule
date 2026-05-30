package ktk

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"log/slog"
	"net/http"
	"net/http/cookiejar"
	"net/url"
	"strconv"
	"strings"
	"sync"
	"time"
)

type Client struct {
	baseURL             string
	device              string
	debugSchedule       bool
	subgroup            string
	teacherHash         string
	teacherScheduleHash string
	httpClient          *http.Client

	endpointsMu sync.RWMutex
	endpoints   Endpoints
}

type Option func(*Client)

type signInRequest struct {
	Login    string `json:"Login"`
	Password string `json:"Password"`
	Device   string `json:"Device"`
}

type accountInfo struct {
	Hash      string `json:"Hash"`
	IsStudent *bool  `json:"IsStudent"`
}

func NewClient(baseURL string, options ...Option) (*Client, error) {
	baseURL = strings.TrimRight(strings.TrimSpace(baseURL), "/")
	if baseURL == "" {
		return nil, fmt.Errorf("base url is empty")
	}
	parsedBaseURL, err := url.Parse(baseURL)
	if err != nil || parsedBaseURL.Scheme == "" || parsedBaseURL.Host == "" {
		return nil, fmt.Errorf("invalid base url: %q", baseURL)
	}

	jar, err := cookiejar.New(nil)
	if err != nil {
		return nil, err
	}

	client := &Client{
		baseURL:   parsedBaseURL.String(),
		device:    "ktk-schedule",
		endpoints: DefaultEndpoints(),
		httpClient: &http.Client{
			Timeout: 15 * time.Second,
			Jar:     jar,
		},
	}

	for _, option := range options {
		option(client)
	}
	client.endpoints = client.endpoints.WithFallback(DefaultEndpoints())

	return client, nil
}

func WithEndpoints(endpoints Endpoints) Option {
	return func(c *Client) {
		c.endpoints = endpoints.WithFallback(c.endpoints)
	}
}

func WithDeviceName(device string) Option {
	return func(c *Client) {
		device = strings.TrimSpace(device)
		if device != "" {
			c.device = device
		}
	}
}

func WithScheduleDebug(enabled bool) Option {
	return func(c *Client) {
		c.debugSchedule = enabled
	}
}

func (c *Client) SignIn(ctx context.Context, login, password string) error {
	body, err := json.Marshal(signInRequest{
		Login:    login,
		Password: password,
		Device:   c.device,
	})
	if err != nil {
		return err
	}

	endpoint := c.endpointSnapshot()
	signInURL, err := c.resolveURL(endpoint.SignInPath)
	if err != nil {
		return err
	}

	req, err := http.NewRequestWithContext(ctx, http.MethodPost, signInURL, bytes.NewReader(body))
	if err != nil {
		return err
	}

	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Accept", "*/*")
	req.Header.Set("Origin", c.baseURL)
	req.Header.Set("Referer", c.baseURL+"/")
	req.Header.Set("User-Agent", "ktk-schedule/1.0")

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	body, _ = readLimitedBody(resp)

	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("sign in failed: %s", resp.Status)
	}
	if subgroup := extractPersonalSubgroup(body); subgroup != "" {
		slog.Debug("subgroup detected", "subgroup", subgroup)
		c.subgroup = subgroup
	}
	if teacherHash := extractTeacherHash(body); teacherHash != "" {
		slog.Debug("teacher hash detected", "teacher_hash", teacherHash)
		c.teacherHash = teacherHash
	}
	scheduleHash := extractTeacherScheduleHash(body)
	if scheduleHash != "" {
		slog.Debug("teacher schedule hash detected", "schedule_hash", scheduleHash)
		c.teacherScheduleHash = scheduleHash
	}

	if c.teacherHash == "" {
		if isTeacherByRole(body) {
			slog.Debug("teacher role detected, searching for hash")
			if hash := extractAnyHash(body); hash != "" {
				slog.Debug("teacher hash extracted via role detection", "hash_prefix", hash[:min(len(hash), 12)])
				c.teacherHash = hash
			}
		}
	}

	if c.teacherHash == "" && c.teacherScheduleHash != "" {
		slog.Debug("using teacher schedule hash as teacher identifier")
		c.teacherHash = c.teacherScheduleHash
	}

	if info, infoBody, err := c.GetAccountInfo(ctx); err == nil && info.IsStudent != nil {
		if *info.IsStudent {
			slog.Debug("account info detected student")
			c.teacherHash = ""
		} else {
			slog.Debug("account info detected teacher")
			if info.Hash != "" {
				c.teacherHash = info.Hash
			} else if teacherHash := extractTeacherHash(infoBody); teacherHash != "" {
				c.teacherHash = teacherHash
			}
			if c.teacherHash == "" {
				if scheduleHash := extractTeacherScheduleHash(infoBody); scheduleHash != "" {
					c.teacherHash = scheduleHash
				}
			}
			if c.teacherHash == "" {
				c.teacherHash = "teacher"
			}
		}
	} else if err != nil {
		slog.Warn("account info unavailable", "error", err)
	} else {
		slog.Warn("account info response has no IsStudent field")
	}

	if c.debugSchedule {
		preview := string(body)
		if len(preview) > 2000 {
			preview = preview[:2000]
		}
		slog.Debug("sign in response body", "body", preview)
	}

	slog.Info("sign in successful", "teacher", c.teacherHash != "")
	return nil
}

func (c *Client) GetAccountInfo(ctx context.Context) (accountInfo, []byte, error) {
	paths := c.cachedAccountInfoPaths()
	var err error
	if len(paths) > 0 {
		info, body, infoErr := c.firstValidAccountInfo(ctx, paths)
		if infoErr == nil {
			return info, body, nil
		}
		err = infoErr
	}

	discoveredPaths, discoveryErr := c.discoveredAccountInfoPaths(ctx)
	if discoveryErr != nil && len(paths) == 0 {
		return accountInfo{}, nil, discoveryErr
	}
	checked := len(paths)
	paths = appendInfoPaths(paths, discoveredPaths...)
	if len(paths) > checked {
		info, body, retryErr := c.firstValidAccountInfo(ctx, paths[checked:])
		if retryErr == nil {
			return info, body, nil
		}
		return accountInfo{}, nil, retryErr
	}
	if err != nil {
		return accountInfo{}, nil, err
	}
	if discoveryErr != nil {
		return accountInfo{}, nil, discoveryErr
	}
	return accountInfo{}, nil, fmt.Errorf("account info endpoint not found")
}

func (c *Client) firstValidAccountInfo(ctx context.Context, paths []string) (accountInfo, []byte, error) {
	if len(paths) == 0 {
		return accountInfo{}, nil, nil
	}

	var lastErr error
	for _, path := range paths {
		info, body, err := c.getAccountInfo(ctx, path)
		if err != nil {
			lastErr = err
			continue
		}
		if info.IsStudent == nil {
			lastErr = fmt.Errorf("account info response has no IsStudent field")
			continue
		}
		endpoint := c.endpointSnapshot()
		endpoint.InfoPath = path
		c.setEndpoints(endpoint)
		return info, body, nil
	}
	return accountInfo{}, nil, lastErr
}

func (c *Client) getAccountInfo(ctx context.Context, path string) (accountInfo, []byte, error) {
	infoURL, err := c.resolveURL(path)
	if err != nil {
		return accountInfo{}, nil, err
	}

	var info accountInfo
	var body []byte
	err = retryGet(ctx, 3, func(retryCtx context.Context) error {
		req, err := http.NewRequestWithContext(retryCtx, http.MethodGet, infoURL, nil)
		if err != nil {
			return err
		}
		req.Header.Set("Accept", "application/json,*/*")
		req.Header.Set("Content-Type", "application/json; charset=utf-8")
		req.Header.Set("Referer", c.baseURL+"/")
		req.Header.Set("User-Agent", "ktk-schedule/1.0")
		req.Header.Set("X-Requested-With", "XMLHttpRequest")

		resp, err := c.httpClient.Do(req)
		if err != nil {
			return err
		}
		defer resp.Body.Close()

		body, _ = readLimitedBody(resp)
		if resp.StatusCode != http.StatusOK {
			return endpointError{operation: "account info", statusCode: resp.StatusCode, status: resp.Status, body: string(body)}
		}
		if err := json.Unmarshal(body, &info); err != nil {
			return endpointError{operation: "account info", statusCode: resp.StatusCode, status: resp.Status, body: string(body), err: err}
		}
		return nil
	})
	return info, body, err
}

func (c *Client) cachedAccountInfoPaths() []string {
	endpoint := c.endpointSnapshot()
	paths := make([]string, 0, 8)
	if endpoint.InfoPath != "" {
		paths = appendInfoPaths(paths, endpoint.InfoPath)
	}
	if path := DeriveInfoPath(endpoint.SchedulePath); path != "" {
		paths = appendInfoPaths(paths, path)
	}
	if path := DeriveInfoPath(endpoint.CallPresetPath); path != "" {
		paths = appendInfoPaths(paths, path)
	}
	return paths
}

func (c *Client) discoveredAccountInfoPaths(ctx context.Context) ([]string, error) {
	candidates, err := c.discoverEndpointCandidates(ctx)
	if err != nil {
		return nil, err
	}
	paths := make([]string, 0, len(candidates.infoPaths)+len(candidates.schedulePaths)+len(candidates.callPresetPaths))
	for _, path := range candidates.infoPaths {
		paths = appendInfoPaths(paths, path)
	}
	for _, path := range candidates.schedulePaths {
		if infoPath := DeriveInfoPath(path); infoPath != "" {
			paths = appendInfoPaths(paths, infoPath)
		}
	}
	for _, path := range candidates.callPresetPaths {
		if infoPath := DeriveInfoPath(path); infoPath != "" {
			paths = appendInfoPaths(paths, infoPath)
		}
	}
	return paths, nil
}

func appendInfoPaths(paths []string, values ...string) []string {
	for _, value := range values {
		paths = appendUnique(paths, value)
	}
	return paths
}

func (c *Client) Subgroup() string {
	return c.subgroup
}

func (c *Client) TeacherHash() string {
	return c.teacherHash
}

func (c *Client) TeacherScheduleHash() string {
	return c.teacherScheduleHash
}

func (c *Client) endpointSnapshot() Endpoints {
	c.endpointsMu.RLock()
	defer c.endpointsMu.RUnlock()
	return c.endpoints
}

func (c *Client) Endpoints() Endpoints {
	return c.endpointSnapshot()
}

func (c *Client) setEndpoints(endpoints Endpoints) {
	c.endpointsMu.Lock()
	defer c.endpointsMu.Unlock()
	c.endpoints = endpoints.WithFallback(c.endpoints).WithFallback(DefaultEndpoints())
}

func (c *Client) resolveURL(path string) (string, error) {
	path = strings.TrimSpace(path)
	if path == "" {
		return "", fmt.Errorf("empty endpoint path")
	}

	baseURL, err := url.Parse(c.baseURL + "/")
	if err != nil {
		return "", err
	}
	reference, err := url.Parse(path)
	if err != nil {
		return "", err
	}

	return baseURL.ResolveReference(reference).String(), nil
}

func extractPersonalSubgroup(body []byte) string {
	var value any
	if err := json.Unmarshal(body, &value); err != nil {
		return ""
	}
	return findPersonalSubgroup(value)
}

func findPersonalSubgroup(value any) string {
	switch typed := value.(type) {
	case map[string]any:
		for key, raw := range typed {
			if strings.Contains(strings.ToLower(key), "subgroup") {
				if subgroup := subgroupFromValue(raw); subgroup != "" {
					return subgroup
				}
			}
		}
		for _, raw := range typed {
			if subgroup := findPersonalSubgroup(raw); subgroup != "" {
				return subgroup
			}
		}
	case []any:
		for _, raw := range typed {
			if subgroup := findPersonalSubgroup(raw); subgroup != "" {
				return subgroup
			}
		}
	}
	return ""
}

func subgroupFromValue(value any) string {
	switch typed := value.(type) {
	case string:
		if subgroup, ok := ParsePersonalSubgroup(typed); ok {
			return subgroup
		}
	case float64:
		if subgroup, ok := ParsePersonalSubgroup(strconv.Itoa(int(typed))); ok {
			return subgroup
		}
	}
	return ""
}

func extractTeacherHash(body []byte) string {
	var value any
	if err := json.Unmarshal(body, &value); err != nil {
		return ""
	}
	return findTeacherHash(value)
}

func extractAnyHash(body []byte) string {
	var value any
	if err := json.Unmarshal(body, &value); err != nil {
		return ""
	}
	return findAnyHash(value)
}

func extractTeacherScheduleHash(body []byte) string {
	var value any
	if err := json.Unmarshal(body, &value); err != nil {
		return ""
	}
	return findTeacherScheduleHash(value)
}

func findTeacherScheduleHash(value any) string {
	switch typed := value.(type) {
	case map[string]any:
		for key, raw := range typed {
			lowerKey := strings.ToLower(key)
			if lowerKey == "hash" || lowerKey == "schedulehash" || lowerKey == "schedule_hash" {
				if s, ok := raw.(string); ok && len(s) >= 10 && len(s) <= 20 {
					return s
				}
			}
		}
		for _, raw := range typed {
			if hash := findTeacherScheduleHash(raw); hash != "" {
				return hash
			}
		}
	case []any:
		for _, raw := range typed {
			if hash := findTeacherScheduleHash(raw); hash != "" {
				return hash
			}
		}
	}
	return ""
}

func findTeacherHash(value any) string {
	// First pass: look for keys explicitly containing "teacherhash" or "teacher_hash"
	if hash := findTeacherHashExact(value); hash != "" {
		return hash
	}
	// Second pass: look for any key with "hash" and a long string value
	if hash := findAnyHash(value); hash != "" {
		return hash
	}
	return ""
}

func findTeacherHashExact(value any) string {
	switch typed := value.(type) {
	case map[string]any:
		for key, raw := range typed {
			lowerKey := strings.ToLower(key)
			if strings.Contains(lowerKey, "teacherhash") || strings.Contains(lowerKey, "teacher_hash") {
				if s, ok := raw.(string); ok && len(s) > 20 {
					return s
				}
			}
		}
		for _, raw := range typed {
			if hash := findTeacherHashExact(raw); hash != "" {
				return hash
			}
		}
	case []any:
		for _, raw := range typed {
			if hash := findTeacherHashExact(raw); hash != "" {
				return hash
			}
		}
	}
	return ""
}

func findAnyHash(value any) string {
	switch typed := value.(type) {
	case map[string]any:
		for key, raw := range typed {
			lowerKey := strings.ToLower(key)
			if strings.Contains(lowerKey, "hash") || lowerKey == "teacher" {
				if s, ok := raw.(string); ok && len(s) > 20 {
					return s
				}
			}
		}
		for _, raw := range typed {
			if hash := findAnyHash(raw); hash != "" {
				return hash
			}
		}
	case []any:
		for _, raw := range typed {
			if hash := findAnyHash(raw); hash != "" {
				return hash
			}
		}
	}
	return ""
}

func isTeacherByRole(body []byte) bool {
	var value any
	if err := json.Unmarshal(body, &value); err != nil {
		return false
	}
	return findTeacherRole(value)
}

func findTeacherRole(value any) bool {
	switch typed := value.(type) {
	case map[string]any:
		for key, raw := range typed {
			lowerKey := strings.ToLower(key)
			if lowerKey == "role" || lowerKey == "type" || lowerKey == "usertype" || lowerKey == "accounttype" {
				if s, ok := raw.(string); ok {
					sl := strings.ToLower(s)
					if sl == "teacher" || sl == "преподаватель" || sl == "admin" || sl == "администратор" {
						return true
					}
				}
			}
		}
		for _, raw := range typed {
			if findTeacherRole(raw) {
				return true
			}
		}
	case []any:
		for _, raw := range typed {
			if findTeacherRole(raw) {
				return true
			}
		}
	}
	return false
}
