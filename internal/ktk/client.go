package ktk

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/cookiejar"
	"net/url"
	"strconv"
	"strings"
	"sync"
	"time"
)

type Client struct {
	baseURL       string
	device        string
	debugSchedule bool
	subgroup      string
	httpClient    *http.Client

	endpointsMu sync.RWMutex
	endpoints   Endpoints
}

type Option func(*Client)

type signInRequest struct {
	Login    string `json:"Login"`
	Password string `json:"Password"`
	Device   string `json:"Device"`
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

	req.Header.Set("Content-Type", "text/plain;charset=UTF-8")
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
		c.subgroup = subgroup
	}

	return nil
}

func (c *Client) Subgroup() string {
	return c.subgroup
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
