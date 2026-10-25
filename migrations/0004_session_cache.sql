-- Session cache (HTTP cookies, session ID) to reuse a single active session across restarts.
-- One row per account; cleared on logout or re-authentication.
CREATE TABLE session_cache (
    telegram_id INTEGER PRIMARY KEY REFERENCES accounts(telegram_id) ON DELETE CASCADE,
    data TEXT NOT NULL,
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);
