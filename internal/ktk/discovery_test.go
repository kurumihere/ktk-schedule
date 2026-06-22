package ktk

import (
	"context"
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestGetScheduleRefreshesStaleEndpointFromWorkspaceAssets(t *testing.T) {
	const weekMillis int64 = 1777240800000

	server := httptest.NewServer(http.HandlerFunc(discoveryTestMockServer))
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

func TestRefreshEndpointsSeparatesPersonalAndGroupSchedules(t *testing.T) {
	const weekMillis int64 = 1777240800000

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/":
			_, _ = w.Write([]byte(`<html><body><script type="module" src="/assets/app.js"></script></body></html>`))
		case "/assets/app.js":
			_, _ = w.Write([]byte(`
				const studentSchedule = "/v0/root/tenant/student-schedule";
				const groupSchedule = "/v0/root/tenant/group-schedule";
			`))
		case "/v0/root/tenant/student-schedule":
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte(`[{"Date":"2026-04-27T00:00:00Z","Subjects":[{"Appraisal":5,"Discipline":"Personal","LectureHall":1,"Pair":1,"Subgroup":"middle"}]}]`))
		case "/v0/root/tenant/group-schedule":
			if r.URL.Query().Get("Group") == "268" {
				w.Header().Set("Content-Type", "application/json")
				_, _ = w.Write([]byte(`[{"Date":"2026-04-27T00:00:00Z","Subjects":[{"Discipline":"Other group","LectureHall":2,"Pair":1,"Subgroup":"1-я подгруппа"}]}]`))
				return
			}
			if r.URL.Query().Get("Group") != "269" {
				http.Error(w, "bad group", http.StatusBadRequest)
				return
			}
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte(`[{"Date":"2026-04-27T00:00:00Z","Subjects":[{"Discipline":"Group left","LectureHall":2,"Pair":1,"Subgroup":"1-я подгруппа"},{"Discipline":"Group right","LectureHall":3,"Pair":1,"Subgroup":"2-я подгруппа"}]}]`))
		default:
			http.NotFound(w, r)
		}
	}))
	defer server.Close()

	client, err := NewClient(server.URL)
	if err != nil {
		t.Fatal(err)
	}
	if err := client.RefreshEndpoints(context.Background(), 269, weekMillis); err != nil {
		t.Fatal(err)
	}

	endpoints := client.Endpoints()
	if endpoints.SchedulePath != "/v0/root/tenant/student-schedule" {
		t.Fatalf("unexpected personal schedule path: %s", endpoints.SchedulePath)
	}
	if endpoints.GroupSchedulePath != "/v0/root/tenant/group-schedule" {
		t.Fatalf("unexpected group schedule path: %s", endpoints.GroupSchedulePath)
	}

	personal, err := client.GetSchedule(context.Background(), 269, weekMillis)
	if err != nil {
		t.Fatal(err)
	}
	if got := personal[0].Subjects[0].Discipline; got != "Personal" {
		t.Fatalf("unexpected personal schedule subject: %s", got)
	}

	group, err := client.GetGroupSchedule(context.Background(), 269, weekMillis)
	if err != nil {
		t.Fatal(err)
	}
	if len(group[0].Subjects) != 2 {
		t.Fatalf("unexpected group schedule subjects: %#v", group[0].Subjects)
	}
}

func TestRefreshEndpointsUpdatesFileAndHomeworkHashes(t *testing.T) {
	const weekMillis int64 = 1777240800000

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/":
			_, _ = w.Write([]byte(`<html><body><script type="module" src="/assets/app.js"></script></body></html>`))
		case "/assets/app.js":
			_, _ = w.Write([]byte(`
				const schedule = "/v0/root/tenant/schedule";
				const homework = "/v0/root/homework-new/homework/check";
				const fileID = "/v0/root/file-new/id?ID=175294";
				const fileOpen = "/v0/root/file-new/open?ID=175294";
			`))
		case "/old/stale-schedule":
			http.NotFound(w, r)
		case "/v0/root/tenant/schedule":
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte(`[{"Date":"2026-04-27T00:00:00Z","Subjects":[{"Discipline":"Math","LectureHall":42,"Pair":1}]}]`))
		default:
			http.NotFound(w, r)
		}
	}))
	defer server.Close()

	client, err := NewClient(server.URL, WithEndpoints(Endpoints{
		SchedulePath: "/old/stale-schedule",
		FileHash:     "file-old",
		HomeworkHash: "homework-old",
	}))
	if err != nil {
		t.Fatal(err)
	}

	if _, err := client.GetSchedule(context.Background(), 269, weekMillis); err != nil {
		t.Fatal(err)
	}

	endpoints := client.Endpoints()
	if endpoints.FileHash != "file-new" {
		t.Fatalf("expected refreshed file hash, got: %s", endpoints.FileHash)
	}
	if endpoints.HomeworkHash != "homework-new" {
		t.Fatalf("expected refreshed homework hash, got: %s", endpoints.HomeworkHash)
	}
}

func TestGetFileLinkRefreshesStaleFileHash(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/":
			_, _ = w.Write([]byte(`<html><body><script type="module" src="/assets/app.js"></script></body></html>`))
		case "/assets/app.js":
			_, _ = w.Write([]byte(`const fileOpen = "/v0/root/file-new/open?ID=175294";`))
		case "/v0/root/file-old/open":
			http.NotFound(w, r)
		case "/v0/root/file-new/open":
			if r.URL.Query().Get("ID") != "175294" {
				http.Error(w, "bad file id", http.StatusBadRequest)
				return
			}
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte(`{"Caption":"plan.rar","Link":"/HEAP/file.rar"}`))
		default:
			http.NotFound(w, r)
		}
	}))
	defer server.Close()

	client, err := NewClient(server.URL, WithEndpoints(Endpoints{
		SchedulePath: "/v0/root/tenant/schedule",
		FileHash:     "file-old",
	}))
	if err != nil {
		t.Fatal(err)
	}

	link, caption, err := client.GetFileLink(context.Background(), 175294)
	if err != nil {
		t.Fatal(err)
	}
	if link != server.URL+"/HEAP/file.rar" || caption != "plan.rar" {
		t.Fatalf("unexpected file link: link=%q caption=%q", link, caption)
	}
	if client.Endpoints().FileHash != "file-new" {
		t.Fatalf("expected refreshed file hash, got: %s", client.Endpoints().FileHash)
	}
}

func TestTeacherScheduleEndpointDoesNotRequireGrades(t *testing.T) {
	const weekMillis int64 = 1777240800000

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/":
			_, _ = w.Write([]byte(`<html><body><script type="module" src="/assets/app.js"></script></body></html>`))
		case "/assets/app.js":
			_, _ = w.Write([]byte(`
				const emptyScheduleWithGrades = "/v0/root/tenant/student-grades-schedule";
				const teacherSchedule = "/v0/root/tenant/teacher-schedule";
			`))
		case "/v0/root/tenant/student-grades-schedule":
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte(`[{"Date":"2026-04-27T00:00:00Z","Subjects":[{"Appraisal":5,"Discipline":"Old","LectureHall":1,"Pair":1}]}]`))
		case "/v0/root/tenant/teacher-schedule":
			if r.URL.Query().Get("Teacher") != "teacher-hash" || r.URL.Query().Get("Group") != "" {
				http.Error(w, "bad teacher schedule query", http.StatusBadRequest)
				return
			}
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte(`[{"Date":"2026-04-27T00:00:00Z","Subjects":[{"Discipline":"Math","Group":"269","LectureHall":42,"Pair":1},{"Discipline":"PE","Group":"270","LectureHall":43,"Pair":2}]}]`))
		default:
			http.NotFound(w, r)
		}
	}))
	defer server.Close()

	client, err := NewClient(server.URL)
	if err != nil {
		t.Fatal(err)
	}

	if err := client.RefreshEndpoints(context.Background(), 0, weekMillis, "teacher-hash"); err != nil {
		t.Fatal(err)
	}
	if client.Endpoints().SchedulePath != "/v0/root/tenant/teacher-schedule" {
		t.Fatalf("unexpected schedule path: %s", client.Endpoints().SchedulePath)
	}
}

func discoveryTestMockServer(w http.ResponseWriter, r *http.Request) {
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
}

func discoveryTestMockServerPersonal(w http.ResponseWriter, r *http.Request) {
	switch r.URL.Path {
	case "/":
		_, _ = w.Write([]byte(`<html><body><script src="/assets/app.js"></script></body></html>`))
	case "/assets/app.js":
		_, _ = w.Write([]byte(`
			const s1 = "/v0/schedule-bad";
			const s2 = "/v0/schedule-group";
			const s3 = "/v0/schedule-personal";
		`))
	case "/v0/schedule-bad":
		http.Error(w, "error", http.StatusInternalServerError)
	case "/v0/schedule-group":
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`[{"Date":"2026-04-27T00:00:00Z","Subjects":[
			{"Discipline":"Math","Teacher":"Teacher","Pair":1,"Mark":0}
		]}]`))
	case "/v0/schedule-personal":
		if r.URL.Query().Get("TeacherHash") == "" {
			http.Error(w, "need teacher hash", http.StatusBadRequest)
			return
		}
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`[{"Date":"2026-04-27T00:00:00Z","Subjects":[
			{"Discipline":"Math","Teacher":"Teacher","Pair":1,"Mark":5}
		]}]`))
	default:
		http.NotFound(w, r)
	}
}

func TestDiscovery(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(discoveryTestMockServerPersonal))
	defer server.Close()
	// ...
}
