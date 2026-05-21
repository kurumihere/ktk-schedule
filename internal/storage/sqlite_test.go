package storage

import (
	"path/filepath"
	"testing"

	credentialspkg "ktk-schedule/internal/credentials"
)

const storageTestSecret = "storage-test-secret-with-32-characters"

func TestSaveUserEncryptsPassword(t *testing.T) {
	store := newTestStorage(t)
	defer store.Close()

	user := User{
		TelegramID:       1001,
		Login:            "student",
		Password:         "student-password",
		GroupID:          269,
		Notify:           true,
		Subgroup:         "right",
		ShowAllSubgroups: true,
	}
	if err := store.SaveUser(user); err != nil {
		t.Fatal(err)
	}

	rawPassword := rawStoredPassword(t, store, user.TelegramID)
	if rawPassword == user.Password {
		t.Fatal("password was stored as plaintext")
	}
	if !credentialspkg.IsEncrypted(rawPassword) {
		t.Fatal("password was not stored in encrypted format")
	}

	saved, err := store.GetUser(user.TelegramID)
	if err != nil {
		t.Fatal(err)
	}
	if saved == nil {
		t.Fatal("user was not found")
	}
	if saved.Password != user.Password {
		t.Fatalf("unexpected password: %q", saved.Password)
	}
	if saved.PasswordLegacy {
		t.Fatal("newly saved user must not be marked as legacy")
	}
	if !saved.Notify {
		t.Fatal("notify flag was not restored")
	}
	if saved.Subgroup != user.Subgroup {
		t.Fatalf("unexpected subgroup: %q", saved.Subgroup)
	}
	if !saved.ShowAllSubgroups {
		t.Fatal("show all subgroups flag was not restored")
	}
}

func TestLegacyPlaintextPasswordMigration(t *testing.T) {
	store := newTestStorage(t)
	defer store.Close()

	_, err := store.db.Exec(`
	INSERT INTO users (telegram_id, login, password, group_id, notify)
	VALUES (?, ?, ?, ?, ?);
	`, int64(1002), "student", "legacy-password", 269, 1)
	if err != nil {
		t.Fatal(err)
	}

	user, err := store.GetUser(1002)
	if err != nil {
		t.Fatal(err)
	}
	if user == nil {
		t.Fatal("user was not found")
	}
	if user.Password != "legacy-password" {
		t.Fatalf("unexpected legacy password: %q", user.Password)
	}
	if !user.PasswordLegacy {
		t.Fatal("plaintext password must be marked as legacy")
	}

	if err := store.SaveUser(*user); err != nil {
		t.Fatal(err)
	}

	rawPassword := rawStoredPassword(t, store, user.TelegramID)
	if rawPassword == "legacy-password" {
		t.Fatal("legacy password was not migrated")
	}
	if !credentialspkg.IsEncrypted(rawPassword) {
		t.Fatal("migrated password was not stored in encrypted format")
	}

	migrated, err := store.GetUser(user.TelegramID)
	if err != nil {
		t.Fatal(err)
	}
	if migrated.Password != "legacy-password" {
		t.Fatalf("unexpected migrated password: %q", migrated.Password)
	}
	if migrated.PasswordLegacy {
		t.Fatal("migrated user must not be marked as legacy")
	}
	if migrated.Subgroup != "left" {
		t.Fatalf("unexpected migrated subgroup: %q", migrated.Subgroup)
	}
}

func TestSubgroupSettings(t *testing.T) {
	store := newTestStorage(t)
	defer store.Close()

	user := User{
		TelegramID: 1003,
		Login:      "student",
		Password:   "student-password",
		GroupID:    269,
		Subgroup:   "left",
	}
	if err := store.SaveUser(user); err != nil {
		t.Fatal(err)
	}
	if err := store.SetShowAllSubgroups(user.TelegramID, true); err != nil {
		t.Fatal(err)
	}
	if err := store.SetSubgroup(user.TelegramID, "right"); err != nil {
		t.Fatal(err)
	}

	saved, err := store.GetUser(user.TelegramID)
	if err != nil {
		t.Fatal(err)
	}
	if saved.Subgroup != "right" {
		t.Fatalf("unexpected subgroup: %q", saved.Subgroup)
	}
	if saved.ShowAllSubgroups {
		t.Fatal("manual subgroup selection must disable all-subgroups mode")
	}
}

func TestListUserIDs(t *testing.T) {
	store := newTestStorage(t)
	defer store.Close()

	for _, id := range []int64{3002, 3001} {
		if err := store.SaveUser(User{
			TelegramID: id,
			Login:      "student",
			Password:   "student-password",
			GroupID:    269,
			Subgroup:   "left",
		}); err != nil {
			t.Fatal(err)
		}
	}

	ids, err := store.ListUserIDs()
	if err != nil {
		t.Fatal(err)
	}
	if len(ids) != 2 || ids[0] != 3001 || ids[1] != 3002 {
		t.Fatalf("unexpected user ids: %#v", ids)
	}
}

func newTestStorage(t *testing.T) *Storage {
	t.Helper()

	cipher, err := credentialspkg.New(storageTestSecret)
	if err != nil {
		t.Fatal(err)
	}

	store, err := New(filepath.Join(t.TempDir(), "test.db"), cipher)
	if err != nil {
		t.Fatal(err)
	}
	return store
}

func rawStoredPassword(t *testing.T, store *Storage, telegramID int64) string {
	t.Helper()

	var password string
	err := store.db.QueryRow(`
	SELECT password
	FROM users
	WHERE telegram_id = ?;
	`, telegramID).Scan(&password)
	if err != nil {
		t.Fatal(err)
	}
	return password
}

func TestListNotifyUsersReturnsOnlyEnabled(t *testing.T) {
	store := newTestStorage(t)
	defer store.Close()

	for _, u := range []User{
		{TelegramID: 1, Login: "a", Password: "p", GroupID: 1, Notify: true, Subgroup: "left"},
		{TelegramID: 2, Login: "b", Password: "p", GroupID: 2, Notify: false, Subgroup: "left"},
		{TelegramID: 3, Login: "c", Password: "p", GroupID: 3, Notify: true, Subgroup: "right"},
	} {
		if err := store.SaveUser(u); err != nil {
			t.Fatal(err)
		}
	}

	users, err := store.ListNotifyUsers()
	if err != nil {
		t.Fatal(err)
	}
	if len(users) != 2 {
		t.Fatalf("expected 2 notify users, got %d", len(users))
	}
	for _, u := range users {
		if !u.Notify {
			t.Fatalf("user %d has notify=false", u.TelegramID)
		}
	}
}

func TestCountUsersAndNotifyUsers(t *testing.T) {
	store := newTestStorage(t)
	defer store.Close()

	for _, u := range []User{
		{TelegramID: 10, Login: "a", Password: "p", GroupID: 1, Notify: true, Subgroup: "left"},
		{TelegramID: 20, Login: "b", Password: "p", GroupID: 2, Notify: false, Subgroup: "left"},
		{TelegramID: 30, Login: "c", Password: "p", GroupID: 3, Notify: true, Subgroup: "right"},
		{TelegramID: 40, Login: "d", Password: "p", GroupID: 4, Notify: false, Subgroup: "left"},
	} {
		if err := store.SaveUser(u); err != nil {
			t.Fatal(err)
		}
	}

	total, err := store.CountUsers()
	if err != nil {
		t.Fatal(err)
	}
	if total != 4 {
		t.Fatalf("expected 4 total users, got %d", total)
	}

	notify, err := store.CountNotifyUsers()
	if err != nil {
		t.Fatal(err)
	}
	if notify != 2 {
		t.Fatalf("expected 2 notify users, got %d", notify)
	}
}

func TestSetNotify(t *testing.T) {
	store := newTestStorage(t)
	defer store.Close()

	user := User{TelegramID: 100, Login: "a", Password: "p", GroupID: 1, Notify: false, Subgroup: "left"}
	if err := store.SaveUser(user); err != nil {
		t.Fatal(err)
	}

	if err := store.SetNotify(100, true); err != nil {
		t.Fatal(err)
	}

	saved, err := store.GetUser(100)
	if err != nil {
		t.Fatal(err)
	}
	if !saved.Notify {
		t.Fatal("notify was not enabled")
	}

	if err := store.SetNotify(100, false); err != nil {
		t.Fatal(err)
	}

	saved, err = store.GetUser(100)
	if err != nil {
		t.Fatal(err)
	}
	if saved.Notify {
		t.Fatal("notify was not disabled")
	}
}

func TestGetUserReturnsNilForUnknown(t *testing.T) {
	store := newTestStorage(t)
	defer store.Close()

	user, err := store.GetUser(99999)
	if err != nil {
		t.Fatal(err)
	}
	if user != nil {
		t.Fatal("expected nil for unknown user")
	}
}

func TestSaveUserUpsertsExisting(t *testing.T) {
	store := newTestStorage(t)
	defer store.Close()

	user := User{TelegramID: 500, Login: "old", Password: "p1", GroupID: 1, Subgroup: "left"}
	if err := store.SaveUser(user); err != nil {
		t.Fatal(err)
	}

	user.Login = "new"
	user.GroupID = 2
	user.Subgroup = "right"
	if err := store.SaveUser(user); err != nil {
		t.Fatal(err)
	}

	saved, err := store.GetUser(500)
	if err != nil {
		t.Fatal(err)
	}
	if saved.Login != "new" {
		t.Fatalf("expected login 'new', got %q", saved.Login)
	}
	if saved.GroupID != 2 {
		t.Fatalf("expected group 2, got %d", saved.GroupID)
	}
	if saved.Subgroup != "right" {
		t.Fatalf("expected subgroup 'right', got %q", saved.Subgroup)
	}
}
