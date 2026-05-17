package storage

import (
	"database/sql"
	"errors"
	"fmt"
	"log/slog"
	"regexp"

	"ktk-schedule/internal/credentials"

	_ "modernc.org/sqlite"
)

type Storage struct {
	db          *sql.DB
	credentials *credentials.Cipher
}

type User struct {
	TelegramID       int64
	Login            string
	Password         string
	GroupID          int
	Notify           bool
	Subgroup         string
	ShowAllSubgroups bool

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
	if _, err := s.db.Exec("PRAGMA journal_mode=WAL"); err != nil {
		slog.Warn("failed to set journal mode", "error", err)
	}
	if _, err := s.db.Exec("PRAGMA busy_timeout=5000"); err != nil {
		slog.Warn("failed to set busy timeout", "error", err)
	}

	if _, err := s.db.Exec(`
	CREATE TABLE IF NOT EXISTS users (
		telegram_id INTEGER PRIMARY KEY,
		login TEXT NOT NULL,
		password TEXT NOT NULL,
		group_id INTEGER NOT NULL,
		notify INTEGER NOT NULL DEFAULT 0
	);
	`); err != nil {
		return err
	}

	if err := s.addColumnIfMissing("users", "subgroup", "subgroup TEXT NOT NULL DEFAULT 'left'"); err != nil {
		return err
	}
	if err := s.addColumnIfMissing("users", "show_all_subgroups", "show_all_subgroups INTEGER NOT NULL DEFAULT 0"); err != nil {
		return err
	}

	if _, err := s.db.Exec(`CREATE INDEX IF NOT EXISTS idx_users_notify ON users (notify)`); err != nil {
		slog.Warn("failed to create notify index", "error", err)
	}
	return nil
}

func (s *Storage) SaveUser(user User) error {
	password, err := s.credentials.Encrypt(user.Password)
	if err != nil {
		return fmt.Errorf("encrypt user password: %w", err)
	}

	_, err = s.db.Exec(`
	INSERT INTO users (telegram_id, login, password, group_id, notify, subgroup, show_all_subgroups)
	VALUES (?, ?, ?, ?, ?, ?, ?)
	ON CONFLICT(telegram_id) DO UPDATE SET
		login = excluded.login,
		password = excluded.password,
		group_id = excluded.group_id,
		subgroup = excluded.subgroup,
		show_all_subgroups = excluded.show_all_subgroups;
	`, user.TelegramID, user.Login, password, user.GroupID, boolToInt(user.Notify), user.Subgroup, boolToInt(user.ShowAllSubgroups))

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

func (s *Storage) SetSubgroup(telegramID int64, subgroup string) error {
	_, err := s.db.Exec(`
	UPDATE users SET subgroup = ?, show_all_subgroups = 0 WHERE telegram_id = ?;
	`, subgroup, telegramID)
	return err
}

func (s *Storage) SetShowAllSubgroups(telegramID int64, enabled bool) error {
	_, err := s.db.Exec(`
	UPDATE users SET show_all_subgroups = ? WHERE telegram_id = ?;
	`, boolToInt(enabled), telegramID)
	return err
}

func (s *Storage) GetUser(telegramID int64) (*User, error) {
	row := s.db.QueryRow(`
	SELECT telegram_id, login, password, group_id, notify, subgroup, show_all_subgroups
	FROM users
	WHERE telegram_id = ?;
	`, telegramID)

	var user User
	var notify int
	var showAllSubgroups int
	var password string

	err := row.Scan(&user.TelegramID, &user.Login, &password, &user.GroupID, &notify, &user.Subgroup, &showAllSubgroups)
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
	user.ShowAllSubgroups = showAllSubgroups == 1
	return &user, nil
}

func (s *Storage) ListNotifyUsers() ([]User, error) {
	rows, err := s.db.Query(`
	SELECT telegram_id, login, password, group_id, notify, subgroup, show_all_subgroups
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
		var showAllSubgroups int
		var password string

		if err := rows.Scan(&user.TelegramID, &user.Login, &password, &user.GroupID, &notify, &user.Subgroup, &showAllSubgroups); err != nil {
			return nil, err
		}

		password, legacy, err := s.decodePassword(password)
		if err != nil {
			return nil, err
		}

		user.Password = password
		user.PasswordLegacy = legacy
		user.Notify = notify == 1
		user.ShowAllSubgroups = showAllSubgroups == 1
		users = append(users, user)
	}

	return users, rows.Err()
}

func (s *Storage) ListUserIDs() ([]int64, error) {
	rows, err := s.db.Query(`
	SELECT telegram_id
	FROM users
	ORDER BY telegram_id;
	`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var ids []int64
	for rows.Next() {
		var id int64
		if err := rows.Scan(&id); err != nil {
			return nil, err
		}
		ids = append(ids, id)
	}

	return ids, rows.Err()
}

var safeIdentifier = regexp.MustCompile(`^[a-zA-Z_][a-zA-Z0-9_]*$`)

func (s *Storage) addColumnIfMissing(table, column, definition string) error {
	if !safeIdentifier.MatchString(table) {
		return fmt.Errorf("invalid table name: %s", table)
	}
	if !safeIdentifier.MatchString(column) {
		return fmt.Errorf("invalid column name: %s", column)
	}

	rows, err := s.db.Query("PRAGMA table_info(" + table + ");")
	if err != nil {
		return err
	}
	defer rows.Close()

	for rows.Next() {
		var cid int
		var name string
		var dataType string
		var notNull int
		var defaultValue sql.NullString
		var pk int

		if err := rows.Scan(&cid, &name, &dataType, &notNull, &defaultValue, &pk); err != nil {
			return err
		}
		if name == column {
			return rows.Err()
		}
	}
	if err := rows.Err(); err != nil {
		return err
	}

	_, err = s.db.Exec("ALTER TABLE " + table + " ADD COLUMN " + definition + ";")
	return err
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

func (s *Storage) CountUsers() (int, error) {
	var count int
	err := s.db.QueryRow(`SELECT COUNT(*) FROM users;`).Scan(&count)
	return count, err
}

func (s *Storage) CountNotifyUsers() (int, error) {
	var count int
	err := s.db.QueryRow(`SELECT COUNT(*) FROM users WHERE notify = 1;`).Scan(&count)
	return count, err
}
