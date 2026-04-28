package ktk

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/cookiejar"
	"time"
)

type Client struct {
	baseURL string
	http    *http.Client
}

type SignInRequest struct {
	Login    string `json:"Login"`
	Password string `json:"Password"`
	Device   string `json:"Device"`
}

func NewClient(baseURL string) (*Client, error) {
	jar, err := cookiejar.New(nil)
	if err != nil {
		return nil, err
	}

	return &Client{
		baseURL: baseURL,
		http: &http.Client{
			Timeout: 15 * time.Second,
			Jar:     jar,
		},
	}, nil
}

func (c *Client) SignIn(ctx context.Context, login, password string) error {
	body, err := json.Marshal(SignInRequest{
		Login:    login,
		Password: password,
		Device:   "ktk-schedule",
	})
	if err != nil {
		return err
	}

	req, err := http.NewRequestWithContext(ctx, http.MethodPost, c.baseURL+"/sign-in", bytes.NewReader(body))
	if err != nil {
		return err
	}

	req.Header.Set("Content-Type", "text/plain;charset=UTF-8")
	req.Header.Set("Accept", "*/*")
	req.Header.Set("Origin", c.baseURL)
	req.Header.Set("Referer", c.baseURL+"/")
	req.Header.Set("User-Agent", "ktk-schedule/1.0")

	resp, err := c.http.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("sign in failed: %s", resp.Status)
	}

	return nil
}
