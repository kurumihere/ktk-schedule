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
	groupID             int
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

type AccountInfo struct {
	Hash      string `json:"Hash"`
	Group     any    `json:"Group"`
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

	c.processSignInResponse(ctx, body)

	slog.Info("sign in successful", "teacher", c.teacherHash != "")
	return nil
}

func (c *Client) processSignInResponse(ctx context.Context, body []byte) {
	if subgroup := extractPersonalSubgroup(body); subgroup != "" {
		slog.Debug("subgroup detected", "subgroup", subgroup)
		c.subgroup = subgroup
	}
	if groupID := extractPersonalGroupID(body); groupID > 0 {
		slog.Debug("group detected", "group_id", groupID)
		c.groupID = groupID
	}

	if info, infoBody, err := c.GetAccountInfo(ctx); err == nil && info.IsStudent != nil {
		if groupID := accountInfoGroupID(info, infoBody); groupID > 0 {
			slog.Debug("account info detected group", "group_id", groupID)
			c.groupID = groupID
		}
		if *info.IsStudent {
			slog.Debug("account info detected student")
			c.teacherHash = ""
		} else {
			slog.Debug("account info detected teacher")
			if info.Hash != "" {
				c.teacherHash = info.Hash
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
}

func (c *Client) GetAccountInfo(ctx context.Context) (AccountInfo, []byte, error) {
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
		return AccountInfo{}, nil, discoveryErr
	}
	checked := len(paths)
	paths = appendInfoPaths(paths, discoveredPaths...)
	if len(paths) > checked {
		info, body, retryErr := c.firstValidAccountInfo(ctx, paths[checked:])
		if retryErr == nil {
			return info, body, nil
		}
		return AccountInfo{}, nil, retryErr
	}
	if err != nil {
		return AccountInfo{}, nil, err
	}
	if discoveryErr != nil {
		return AccountInfo{}, nil, discoveryErr
	}
	return AccountInfo{}, nil, fmt.Errorf("account info endpoint not found")
}

func (c *Client) firstValidAccountInfo(ctx context.Context, paths []string) (AccountInfo, []byte, error) {
	if len(paths) == 0 {
		return AccountInfo{}, nil, nil
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
	return AccountInfo{}, nil, lastErr
}

func (c *Client) getAccountInfo(ctx context.Context, path string) (AccountInfo, []byte, error) {
	infoURL, err := c.resolveURL(path)
	if err != nil {
		return AccountInfo{}, nil, err
	}

	var info AccountInfo
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
	var seen map[string]struct{}
	for _, value := range values {
		paths, seen = appendUniqueSeen(paths, seen, value)
	}
	return paths
}

func (c *Client) Subgroup() string {
	return c.subgroup
}

func (c *Client) GroupID() int {
	return c.groupID
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

func extractPersonalGroupID(body []byte) int {
	var value any
	if err := json.Unmarshal(body, &value); err != nil {
		return 0
	}
	return findPersonalGroupID(value)
}

func findPersonalGroupID(value any) int {
	switch typed := value.(type) {
	case map[string]any:
		for key, raw := range typed {
			normalized := strings.ToLower(strings.ReplaceAll(key, "_", ""))
			if normalized == "group" || normalized == "groupid" {
				if groupID := groupIDFromValue(raw); groupID > 0 {
					return groupID
				}
			}
		}
		for _, raw := range typed {
			if groupID := findPersonalGroupID(raw); groupID > 0 {
				return groupID
			}
		}
	case []any:
		for _, raw := range typed {
			if groupID := findPersonalGroupID(raw); groupID > 0 {
				return groupID
			}
		}
	}
	return 0
}

func accountInfoGroupID(info AccountInfo, body []byte) int {
	if groupID := groupIDFromValue(info.Group); groupID > 0 {
		return groupID
	}
	return extractPersonalGroupID(body)
}

func groupIDFromValue(value any) int {
	switch typed := value.(type) {
	case string:
		if n, err := strconv.Atoi(strings.TrimSpace(typed)); err == nil && n > 0 {
			return n
		}
	case float64:
		if typed > 0 && typed == float64(int(typed)) {
			return int(typed)
		}
	case json.Number:
		if n, err := typed.Int64(); err == nil && n > 0 {
			return int(n)
		}
	}
	return 0
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
