package storage

import (
	"database/sql"
	"errors"

	_ "modernc.org/sqlite"
)

type Storage struct {
	db *sql.DB
}

type User struct {
	TelegramID int64
	Login      string
	Password   string
	GroupID    int
	Notify     bool
}

func New(path string) (*Storage, error) {
	db, err := sql.Open("sqlite", path)
	if err != nil {
		return nil, err
	}

	s := &Storage{db: db}
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
	_, err := s.db.Exec(`
	INSERT INTO users (telegram_id, login, password, group_id, notify)
	VALUES (?, ?, ?, ?, ?)
	ON CONFLICT(telegram_id) DO UPDATE SET
		login = excluded.login,
		password = excluded.password,
		group_id = excluded.group_id;
	`, user.TelegramID, user.Login, user.Password, user.GroupID, boolToInt(user.Notify))

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

	err := row.Scan(&user.TelegramID, &user.Login, &user.Password, &user.GroupID, &notify)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	}
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

		if err := rows.Scan(&user.TelegramID, &user.Login, &user.Password, &user.GroupID, &notify); err != nil {
			return nil, err
		}

		user.Notify = notify == 1
		users = append(users, user)
	}

	return users, rows.Err()
}

func boolToInt(value bool) int {
	if value {
		return 1
	}
	return 0
}
