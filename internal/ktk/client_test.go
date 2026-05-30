package ktk

import (
	"context"
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestSignInDetectsTeacherFromAccountInfo(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/sign-in":
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte(`{}`))
		case "/":
			_, _ = w.Write([]byte(`<html><body><script src="/assets/app.js"></script></body></html>`))
		case "/assets/app.js":
			_, _ = w.Write([]byte(`const info = "/v0/workspace/infohash123/info";`))
		case "/v0/workspace/infohash123/info":
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte(`{"Hash":"teacherInfoHash1234567890","IsStudent":false}`))
		default:
			http.NotFound(w, r)
		}
	}))
	defer server.Close()

	client, err := NewClient(server.URL)
	if err != nil {
		t.Fatal(err)
	}
	if err := client.SignIn(context.Background(), "teacher", "password"); err != nil {
		t.Fatal(err)
	}

	if client.TeacherHash() != "teacherInfoHash1234567890" {
		t.Fatalf("unexpected teacher hash: %q", client.TeacherHash())
	}
	if client.Endpoints().InfoPath != "/v0/workspace/infohash123/info" {
		t.Fatalf("unexpected info path: %s", client.Endpoints().InfoPath)
	}
}

func TestSignInAccountInfoStudentOverridesHashHeuristic(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/sign-in":
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte(`{"TeacherHash":"teacherHashThatWouldLookValid"}`))
		case "/":
			_, _ = w.Write([]byte(`<html><body><script src="/assets/app.js"></script></body></html>`))
		case "/assets/app.js":
			_, _ = w.Write([]byte(`const info = "/v0/workspace/infohash123/info";`))
		case "/v0/workspace/infohash123/info":
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte(`{"IsStudent":true}`))
		default:
			http.NotFound(w, r)
		}
	}))
	defer server.Close()

	client, err := NewClient(server.URL)
	if err != nil {
		t.Fatal(err)
	}
	if err := client.SignIn(context.Background(), "student", "password"); err != nil {
		t.Fatal(err)
	}

	if client.TeacherHash() != "" {
		t.Fatalf("student account must not be detected as teacher, got hash %q", client.TeacherHash())
	}
}

func TestSignInDoesNotDetectTeacherWithoutAccountInfo(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/sign-in":
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte(`{"TeacherHash":"teacherHashThatWouldLookValid"}`))
		case "/":
			_, _ = w.Write([]byte(`<html><body></body></html>`))
		default:
			http.NotFound(w, r)
		}
	}))
	defer server.Close()

	client, err := NewClient(server.URL)
	if err != nil {
		t.Fatal(err)
	}
	if err := client.SignIn(context.Background(), "unknown", "password"); err != nil {
		t.Fatal(err)
	}

	if client.TeacherHash() != "" {
		t.Fatalf("teacher must be detected only by account info IsStudent, got hash %q", client.TeacherHash())
	}
}

func TestSignInDerivesAccountInfoFromConfiguredSchedulePath(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/sign-in":
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte(`{}`))
		case "/v0/workspace/schedulehash/info":
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte(`{"IsStudent":false}`))
		default:
			http.NotFound(w, r)
		}
	}))
	defer server.Close()

	client, err := NewClient(server.URL, WithEndpoints(Endpoints{
		SchedulePath: "/v0/workspace/schedulehash/schedule",
	}))
	if err != nil {
		t.Fatal(err)
	}
	if err := client.SignIn(context.Background(), "teacher", "password"); err != nil {
		t.Fatal(err)
	}

	if client.TeacherHash() == "" {
		t.Fatal("expected teacher hash marker for non-student account")
	}
	if client.Endpoints().InfoPath != "/v0/workspace/schedulehash/info" {
		t.Fatalf("unexpected info path: %s", client.Endpoints().InfoPath)
	}
}

func TestSignInSkipsBrokenAccountInfoCandidate(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/sign-in":
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte(`{}`))
		case "/":
			_, _ = w.Write([]byte(`<html><body><script src="/assets/app.js"></script></body></html>`))
		case "/assets/app.js":
			_, _ = w.Write([]byte(`
				const brokenInfo = "/v0/workspace/broken/info";
				const goodInfo = "/v0/workspace/good/info";
			`))
		case "/v0/workspace/broken/info":
			http.Error(w, "bad info", http.StatusBadRequest)
		case "/v0/workspace/good/info":
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte(`{"IsStudent":false}`))
		default:
			http.NotFound(w, r)
		}
	}))
	defer server.Close()

	client, err := NewClient(server.URL)
	if err != nil {
		t.Fatal(err)
	}
	if err := client.SignIn(context.Background(), "teacher", "password"); err != nil {
		t.Fatal(err)
	}

	if client.TeacherHash() == "" {
		t.Fatal("expected teacher hash marker for non-student account")
	}
	if client.Endpoints().InfoPath != "/v0/workspace/good/info" {
		t.Fatalf("unexpected info path: %s", client.Endpoints().InfoPath)
	}
}
