-- Reference data (hall names, call presets) for offline schedule rendering.
-- One row per account; refreshed on every online view, cleared on account switch.
CREATE TABLE reference_cache (
    telegram_id INTEGER PRIMARY KEY REFERENCES accounts(telegram_id) ON DELETE CASCADE,
    data TEXT NOT NULL,
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);
