-- Passwords are never stored again. A sign-in keeps only the site session, which the site
-- extends by 30 days on every use; when the site rejects it, the user signs in again.
-- Stored logins and passwords disappear with the old table (Storage::open vacuums the file).
--
-- Tables are rebuilt STRICT and keyed by `user_id`: a Telegram user ID, or a VK user ID
-- plus 10^18 (model::VK_ID_OFFSET). Encrypted payloads are copied unchanged: their
-- associated data names the user ID, not the column.
--
-- Old tables are renamed first and children dropped before the parent, so no
-- ON DELETE CASCADE can reach the copied rows.
ALTER TABLE accounts RENAME TO accounts_v4;
ALTER TABLE schedules RENAME TO schedules_v4;
ALTER TABLE views RENAME TO views_v4;
ALTER TABLE reference_cache RENAME TO reference_cache_v4;
ALTER TABLE session_cache RENAME TO session_cache_v4;
DROP INDEX accounts_notify;

CREATE TABLE accounts (
    user_id INTEGER PRIMARY KEY CHECK (user_id > 0),
    group_id INTEGER NOT NULL CHECK (group_id > 0),
    personal_subgroup TEXT NOT NULL CHECK (personal_subgroup IN ('left', 'right')),
    subgroup TEXT NOT NULL CHECK (subgroup IN ('left', 'right')),
    show_all INTEGER NOT NULL DEFAULT 0 CHECK (show_all IN (0, 1)),
    teacher_hash TEXT NOT NULL DEFAULT '',
    notify INTEGER NOT NULL DEFAULT 0 CHECK (notify IN (0, 1)),
    notified_date TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
) STRICT;
CREATE INDEX accounts_notify ON accounts(user_id) WHERE notify = 1;

-- The signed-in site session (cookies, session ID, endpoints), `cache:v1:` sealed with
-- context ("sessions", user_id). No row means the user has to sign in again.
CREATE TABLE sessions (
    user_id INTEGER PRIMARY KEY REFERENCES accounts(user_id) ON DELETE CASCADE,
    data TEXT NOT NULL,
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
) STRICT;

-- Personal grades and homework must never be shared across accounts.
CREATE TABLE schedules (
    user_id INTEGER NOT NULL REFERENCES accounts(user_id) ON DELETE CASCADE,
    scope TEXT NOT NULL,
    week TEXT NOT NULL,
    data TEXT NOT NULL,
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    PRIMARY KEY (user_id, scope, week)
) STRICT, WITHOUT ROWID;
CREATE INDEX schedules_updated ON schedules(updated_at);

-- Each message owns its navigation state, including after a restart.
CREATE TABLE views (
    user_id INTEGER NOT NULL REFERENCES accounts(user_id) ON DELETE CASCADE,
    message_id INTEGER NOT NULL,
    data TEXT NOT NULL,
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    PRIMARY KEY (user_id, message_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX views_updated ON views(updated_at);

-- Hall names and bell presets for rendering saved weeks while the site is unreachable.
CREATE TABLE reference_cache (
    user_id INTEGER PRIMARY KEY REFERENCES accounts(user_id) ON DELETE CASCADE,
    data TEXT NOT NULL,
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
) STRICT;

INSERT INTO accounts (user_id, group_id, personal_subgroup, subgroup, show_all, teacher_hash, notify, notified_date)
SELECT telegram_id, group_id, personal_subgroup, subgroup, show_all, teacher_hash, notify, notified_date
FROM accounts_v4;
INSERT INTO sessions (user_id, data, updated_at)
SELECT telegram_id, data, updated_at FROM session_cache_v4;
INSERT INTO schedules (user_id, scope, week, data, updated_at)
SELECT telegram_id, scope, week, data, updated_at FROM schedules_v4;
INSERT INTO views (user_id, message_id, data, updated_at)
SELECT telegram_id, message_id, data, updated_at FROM views_v4;
INSERT INTO reference_cache (user_id, data, updated_at)
SELECT telegram_id, data, updated_at FROM reference_cache_v4;

DROP TABLE session_cache_v4;
DROP TABLE reference_cache_v4;
DROP TABLE views_v4;
DROP TABLE schedules_v4;
DROP TABLE accounts_v4;
