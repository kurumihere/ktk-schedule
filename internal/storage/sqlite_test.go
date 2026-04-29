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
		TelegramID: 1001,
		Login:      "student",
		Password:   "student-password",
		GroupID:    269,
		Notify:     true,
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
