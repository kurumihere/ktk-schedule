package storage

import (
	"database/sql"
	"errors"
	"fmt"

	"ktk-schedule/internal/credentials"

	_ "modernc.org/sqlite"
)

type Storage struct {
	db          *sql.DB
	credentials *credentials.Cipher
}

type User struct {
	TelegramID int64
	Login      string
	Password   string
	GroupID    int
	Notify     bool

	PasswordLegacy bool
}

func New(path string, credentialCipher *credentials.Cipher) (*Storage, error) {
	if credentialCipher == nil {
		return nil, errors.New("credential cipher is nil")
	}

	db, err := sql.Open("sqlite", path)
	if err != nil {
		return nil, err
	}

	s := &Storage{db: db, credentials: credentialCipher}
	if err := s.init(); err != nil {
		_ = db.Close()
		return nil, err
	}

	return s, nil
}

func (s *Storage) Close() error {
	return s.db.Close()
}

func (s *Storage) init() error {
	_, err := s.db.Exec(`
	CREATE TABLE IF NOT EXISTS users (
		telegram_id INTEGER PRIMARY KEY,
		login TEXT NOT NULL,
		password TEXT NOT NULL,
		group_id INTEGER NOT NULL,
		notify INTEGER NOT NULL DEFAULT 0
	);
	`)
	return err
}

func (s *Storage) SaveUser(user User) error {
	password, err := s.credentials.Encrypt(user.Password)
	if err != nil {
		return fmt.Errorf("encrypt user password: %w", err)
	}

	_, err = s.db.Exec(`
	INSERT INTO users (telegram_id, login, password, group_id, notify)
	VALUES (?, ?, ?, ?, ?)
	ON CONFLICT(telegram_id) DO UPDATE SET
		login = excluded.login,
		password = excluded.password,
		group_id = excluded.group_id;
	`, user.TelegramID, user.Login, password, user.GroupID, boolToInt(user.Notify))

	return err
}

func (s *Storage) SetGroup(telegramID int64, groupID int) error {
	_, err := s.db.Exec(`
	UPDATE users SET group_id = ? WHERE telegram_id = ?;
	`, groupID, telegramID)
	return err
}

func (s *Storage) SetNotify(telegramID int64, enabled bool) error {
	_, err := s.db.Exec(`
	UPDATE users SET notify = ? WHERE telegram_id = ?;
	`, boolToInt(enabled), telegramID)
	return err
}

func (s *Storage) GetUser(telegramID int64) (*User, error) {
	row := s.db.QueryRow(`
	SELECT telegram_id, login, password, group_id, notify
	FROM users
	WHERE telegram_id = ?;
	`, telegramID)

	var user User
	var notify int
	var password string

	err := row.Scan(&user.TelegramID, &user.Login, &password, &user.GroupID, &notify)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}

	user.Password, user.PasswordLegacy, err = s.decodePassword(password)
	if err != nil {
		return nil, err
	}

	user.Notify = notify == 1
	return &user, nil
}

func (s *Storage) ListNotifyUsers() ([]User, error) {
	rows, err := s.db.Query(`
	SELECT telegram_id, login, password, group_id, notify
	FROM users
	WHERE notify = 1;
	`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var users []User

	for rows.Next() {
		var user User
		var notify int
		var password string

		if err := rows.Scan(&user.TelegramID, &user.Login, &password, &user.GroupID, &notify); err != nil {
			return nil, err
		}

		password, legacy, err := s.decodePassword(password)
		if err != nil {
			return nil, err
		}

		user.Password = password
		user.PasswordLegacy = legacy
		user.Notify = notify == 1
		users = append(users, user)
	}

	return users, rows.Err()
}

func (s *Storage) decodePassword(value string) (string, bool, error) {
	if !credentials.IsEncrypted(value) {
		return value, true, nil
	}

	password, err := s.credentials.Decrypt(value)
	if err != nil {
		return "", false, fmt.Errorf("decrypt user password: %w", err)
	}

	return password, false, nil
}

func boolToInt(value bool) int {
	if value {
		return 1
	}
	return 0
}
