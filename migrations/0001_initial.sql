CREATE TABLE accounts (
    telegram_id INTEGER PRIMARY KEY CHECK (telegram_id > 0),
    login TEXT NOT NULL,
    password TEXT NOT NULL,
    group_id INTEGER NOT NULL CHECK (group_id > 0),
    personal_subgroup TEXT NOT NULL CHECK (personal_subgroup IN ('left', 'right')),
    subgroup TEXT NOT NULL CHECK (subgroup IN ('left', 'right')),
    show_all INTEGER NOT NULL DEFAULT 0 CHECK (show_all IN (0, 1)),
    teacher_hash TEXT NOT NULL DEFAULT '',
    notify INTEGER NOT NULL DEFAULT 0 CHECK (notify IN (0, 1)),
    notified_date TEXT
);
CREATE INDEX accounts_notify ON accounts(telegram_id) WHERE notify = 1;

-- Personal grades and homework must never be shared across accounts.
CREATE TABLE schedules (
    telegram_id INTEGER NOT NULL REFERENCES accounts(telegram_id) ON DELETE CASCADE,
    scope TEXT NOT NULL,
    week TEXT NOT NULL,
    data TEXT NOT NULL,
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    PRIMARY KEY (telegram_id, scope, week)
);

-- Each Telegram message owns its navigation state, including after a restart.
CREATE TABLE views (
    telegram_id INTEGER NOT NULL REFERENCES accounts(telegram_id) ON DELETE CASCADE,
    message_id INTEGER NOT NULL,
    data TEXT NOT NULL,
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    PRIMARY KEY (telegram_id, message_id)
);
