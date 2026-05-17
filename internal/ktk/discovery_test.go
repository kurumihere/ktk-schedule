package ktk

import (
	"context"
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestGetScheduleRefreshesStaleEndpointFromWorkspaceAssets(t *testing.T) {
	const weekMillis int64 = 1777240800000

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/":
			_, _ = w.Write([]byte(`<html><body><script type="module" src="/assets/app.js"></script></body></html>`))
		case "/assets/app.js":
			_, _ = w.Write([]byte(`
				const emptySchedule = "/v0/root/tenant/empty-schedule";
				const schedule = "/v0/root/tenant/schedule-new";
				const halls = "/v0/root/tenant/lecture-hall?Branch=newbranch123";
			`))
		case "/old/schedule", "/old/lecture-hall":
			http.NotFound(w, r)
		case "/v0/root/tenant/empty-schedule":
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte(`[{"Date":"2026-04-27T00:00:00Z","Today":true,"Subjects":[]}]`))
		case "/v0/root/tenant/schedule-new":
			if r.URL.Query().Get("Group") != "269" || r.URL.Query().Get("Week") != "1777240800000" {
				http.Error(w, "bad schedule query", http.StatusBadRequest)
				return
			}
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte(`[{"Date":"2026-04-27T00:00:00Z","Today":true,"Subjects":[{"Discipline":"Math","Teacher":"Teacher","LectureHall":42,"Pair":1}]}]`))
		case "/v0/root/tenant/lecture-hall":
			if r.URL.Query().Get("Branch") != "newbranch123" {
				http.Error(w, "bad branch", http.StatusBadRequest)
				return
			}
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte(`{"LectureHalls":{"1":[{"ID":42,"Housing":1,"Level":2,"Number":"202","Virtual":false}]}}`))
		default:
			http.NotFound(w, r)
		}
	}))
	defer server.Close()

	client, err := NewClient(server.URL, WithEndpoints(Endpoints{
		SchedulePath:    "/old/schedule",
		LectureHallPath: "/old/lecture-hall",
		BranchID:        "oldbranch",
	}))
	if err != nil {
		t.Fatal(err)
	}

	days, err := client.GetSchedule(context.Background(), 269, weekMillis)
	if err != nil {
		t.Fatal(err)
	}
	if len(days) != 1 || !days[0].Today || len(days[0].Subjects) != 1 {
		t.Fatalf("unexpected schedule: %#v", days)
	}

	halls, err := client.GetLectureHalls(context.Background(), 269, weekMillis)
	if err != nil {
		t.Fatal(err)
	}
	if halls[42].Number != "202" {
		t.Fatalf("unexpected lecture halls: %#v", halls)
	}
}

func TestCallPresetDiscoveredFromWorkspaceAssets(t *testing.T) {
	const weekMillis int64 = 1777240800000

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/":
			_, _ = w.Write([]byte(`<html><body><script type="module" src="/assets/app.js"></script></body></html>`))
		case "/assets/app.js":
			_, _ = w.Write([]byte(`
				const schedule = "/v0/root/tenant/schedule";
				const callPreset = "/v0/root/tenant/call-preset";
			`))
		case "/old/stale-schedule":
			http.NotFound(w, r)
		case "/v0/root/tenant/schedule":
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte(`[{"Date":"2026-04-27T00:00:00Z","Today":true,"CallPreset":1,"Subjects":[{"Discipline":"Math","Teacher":"Teacher","LectureHall":42,"Pair":1}]}]`))
		case "/v0/root/tenant/call-preset":
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte(`[{"ID":1,"Name":"Main","Begin":"2026-01-01T08:30:00Z","CallSet":[{"PairNumber":1,"Duration":45,"Break":10}]}]`))
		case "/v0/root/tenant/call-preset-derived":
			http.NotFound(w, r)
		default:
			http.NotFound(w, r)
		}
	}))
	defer server.Close()

	client, err := NewClient(server.URL, WithEndpoints(Endpoints{
		SchedulePath:   "/old/stale-schedule",
		CallPresetPath: "/v0/root/tenant/call-preset-derived",
	}))
	if err != nil {
		t.Fatal(err)
	}

	days, err := client.GetSchedule(context.Background(), 269, weekMillis)
	if err != nil {
		t.Fatal(err)
	}
	if len(days) != 1 {
		t.Fatalf("unexpected schedule length: %d", len(days))
	}

	presets, err := client.GetCallPresets(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if presets[1].Name != "Main" {
		t.Fatalf("unexpected call preset: %#v", presets)
	}

	endpoints := client.Endpoints()
	if endpoints.CallPresetPath != "/v0/root/tenant/call-preset" {
		t.Fatalf("expected discovered call-preset path, got: %s", endpoints.CallPresetPath)
	}
}
