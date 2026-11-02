use crate::{
    credentials::Cipher,
    model::{ReferenceData, ScheduleDay, SessionRecord, View},
};
use anyhow::{Result, ensure};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::{
    Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};
use std::{path::Path, time::Duration};

/// A linked college account: only settings. The site session lives in `sessions`, and no
/// login or password is ever stored.
#[derive(Clone, Debug)]
pub struct Account {
    /// Telegram user ID, or VK user ID + `model::VK_ID_OFFSET`.
    pub id: i64,
    pub group_id: i64,
    pub personal_subgroup: String,
    pub subgroup: String,
    pub show_all: bool,
    pub teacher_hash: String,
    pub notify: bool,
}

pub struct Storage {
    pool: SqlitePool,
    cipher: Cipher,
}

impl Storage {
    pub async fn open(path: &str, cipher: Cipher) -> Result<Self> {
        if path != ":memory:"
            && let Some(parent) = Path::new(path)
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
        {
            tokio::fs::create_dir_all(parent).await?;
        }
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .pragma("secure_delete", "ON")
            .journal_mode(SqliteJournalMode::Wal)
            // With WAL, NORMAL never corrupts the file; a power cut may lose the last commits.
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(5));
        // SQLite serializes writes. One asynchronous worker also avoids pool-level write contention.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        let had_passwords: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pragma_table_info('accounts') WHERE name = 'password')",
        )
        .fetch_one(&pool)
        .await?;
        sqlx::migrate!().run(&pool).await?;
        if had_passwords {
            // Migration 0005 dropped every stored password: rewrite the file so none of
            // them survives in free pages.
            sqlx::query("VACUUM").execute(&pool).await?;
        }
        // Remove checkpointed pages of deleted data from the write-ahead log as well.
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&pool)
            .await?;
        Ok(Self { pool, cipher })
    }

    pub async fn close(&self) {
        self.pool.close().await;
    }

    /// Fail fast when CREDENTIALS_SECRET changed: every saved session would be unreadable.
    pub async fn check_secret(&self) -> Result<()> {
        let rows: Vec<(i64, String)> =
            sqlx::query_as("SELECT user_id, data FROM sessions ORDER BY updated_at DESC LIMIT 16")
                .fetch_all(&self.pool)
                .await?;
        ensure!(
            rows.is_empty()
                || rows
                    .iter()
                    .any(|(id, data)| self.unseal::<SessionRecord>(("sessions", id), data).is_ok()),
            "saved sessions cannot be decrypted: CREDENTIALS_SECRET differs from the one that encrypted them"
        );
        Ok(())
    }

    fn seal(&self, context: impl Serialize, value: &impl Serialize) -> Result<String> {
        self.cipher.encrypt_cache(
            &serde_json::to_string(&context)?,
            &serde_json::to_string(value)?,
        )
    }

    fn unseal<T: DeserializeOwned>(&self, context: impl Serialize, data: &str) -> Result<T> {
        let plain = self
            .cipher
            .decrypt_cache(&serde_json::to_string(&context)?, data)?;
        Ok(serde_json::from_str(&plain)?)
    }

    pub async fn account(&self, id: i64) -> Result<Option<Account>> {
        let Some(row) = sqlx::query(
            "SELECT group_id, personal_subgroup, subgroup, show_all, teacher_hash, notify FROM accounts WHERE user_id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };
        Ok(Some(Account {
            id,
            group_id: row.try_get("group_id")?,
            personal_subgroup: row.try_get("personal_subgroup")?,
            subgroup: row.try_get("subgroup")?,
            show_all: row.try_get("show_all")?,
            teacher_hash: row.try_get("teacher_hash")?,
            notify: row.try_get("notify")?,
        }))
    }

    /// Link an account after a successful sign-in. The notification preference survives a
    /// re-login; everything cached for the previous college account is wiped.
    pub async fn save_account(&self, a: &Account, session: &SessionRecord) -> Result<()> {
        let session = self.seal(("sessions", a.id), session)?;
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO accounts (user_id, group_id, personal_subgroup, subgroup, show_all, teacher_hash, notify) VALUES (?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(user_id) DO UPDATE SET group_id = excluded.group_id, personal_subgroup = excluded.personal_subgroup, \
             subgroup = excluded.subgroup, show_all = excluded.show_all, teacher_hash = excluded.teacher_hash",
        )
        .bind(a.id)
        .bind(a.group_id)
        .bind(&a.personal_subgroup)
        .bind(&a.subgroup)
        .bind(a.show_all)
        .bind(&a.teacher_hash)
        .bind(a.notify)
        .execute(&mut *tx)
        .await?;
        for statement in [
            "DELETE FROM schedules WHERE user_id = ?",
            "DELETE FROM views WHERE user_id = ?",
            "DELETE FROM reference_cache WHERE user_id = ?",
        ] {
            sqlx::query(statement).bind(a.id).execute(&mut *tx).await?;
        }
        sqlx::query(
            "INSERT INTO sessions (user_id, data) VALUES (?, ?) \
             ON CONFLICT(user_id) DO UPDATE SET data = excluded.data, updated_at = unixepoch()",
        )
        .bind(a.id)
        .bind(session)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn set_subgroup(&self, id: i64, subgroup: &str, all: bool) -> Result<()> {
        sqlx::query("UPDATE accounts SET subgroup = ?, show_all = ? WHERE user_id = ?")
            .bind(subgroup)
            .bind(all)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn delete_account(&self, id: i64) -> Result<()> {
        // Foreign keys also remove this user's session, caches and views.
        sqlx::query("DELETE FROM accounts WHERE user_id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn set_notify(&self, id: i64, enabled: bool) -> Result<()> {
        sqlx::query("UPDATE accounts SET notify = ? WHERE user_id = ?")
            .bind(enabled)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn ids(&self) -> Result<Vec<i64>> {
        Ok(
            sqlx::query_scalar("SELECT user_id FROM accounts ORDER BY user_id")
                .fetch_all(&self.pool)
                .await?,
        )
    }

    /// Subscribers not yet notified on `date` who still have a site session.
    pub async fn notification_ids(&self, date: &str) -> Result<Vec<i64>> {
        Ok(sqlx::query_scalar(
            "SELECT a.user_id FROM accounts a JOIN sessions s USING (user_id) \
             WHERE a.notify = 1 AND (a.notified_date IS NULL OR a.notified_date <> ?) ORDER BY a.user_id",
        )
        .bind(date)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn mark_notified(&self, id: i64, date: &str) -> Result<()> {
        sqlx::query("UPDATE accounts SET notified_date = ? WHERE user_id = ?")
            .bind(date)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn counts(&self) -> Result<(i64, i64)> {
        Ok(
            sqlx::query_as("SELECT COUNT(*), COALESCE(SUM(notify), 0) FROM accounts")
                .fetch_one(&self.pool)
                .await?,
        )
    }

    pub async fn save_schedule(
        &self,
        id: i64,
        scope: &str,
        week: &str,
        days: &[ScheduleDay],
    ) -> Result<()> {
        let data = self.seal(("schedules", id, scope, week), &days)?;
        sqlx::query(
            "INSERT INTO schedules (user_id, scope, week, data) VALUES (?, ?, ?, ?) \
             ON CONFLICT(user_id, scope, week) DO UPDATE SET data = excluded.data, updated_at = unixepoch()",
        )
        .bind(id)
        .bind(scope)
        .bind(week)
        .bind(data)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn schedule(
        &self,
        id: i64,
        scope: &str,
        week: &str,
    ) -> Result<Option<Vec<ScheduleDay>>> {
        let data: Option<String> = sqlx::query_scalar(
            "SELECT data FROM schedules WHERE user_id = ? AND scope = ? AND week = ?",
        )
        .bind(id)
        .bind(scope)
        .bind(week)
        .fetch_optional(&self.pool)
        .await?;
        data.map(|data| self.unseal(("schedules", id, scope, week), &data))
            .transpose()
    }

    pub async fn save_references(&self, id: i64, refs: &ReferenceData) -> Result<()> {
        let data = self.seal(("references", id), refs)?;
        sqlx::query(
            "INSERT INTO reference_cache (user_id, data) VALUES (?, ?) \
             ON CONFLICT(user_id) DO UPDATE SET data = excluded.data, updated_at = unixepoch()",
        )
        .bind(id)
        .bind(data)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn references(&self, id: i64) -> Result<Option<ReferenceData>> {
        let data: Option<String> =
            sqlx::query_scalar("SELECT data FROM reference_cache WHERE user_id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        data.map(|data| self.unseal(("references", id), &data))
            .transpose()
    }

    pub async fn save_session(&self, id: i64, session: &SessionRecord) -> Result<()> {
        let data = self.seal(("sessions", id), session)?;
        sqlx::query(
            "INSERT INTO sessions (user_id, data) VALUES (?, ?) \
             ON CONFLICT(user_id) DO UPDATE SET data = excluded.data, updated_at = unixepoch()",
        )
        .bind(id)
        .bind(data)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn session(&self, id: i64) -> Result<Option<SessionRecord>> {
        let data: Option<String> =
            sqlx::query_scalar("SELECT data FROM sessions WHERE user_id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        data.map(|data| self.unseal(("sessions", id), &data))
            .transpose()
    }

    pub async fn delete_session(&self, id: i64) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE user_id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn save_view(&self, id: i64, message: i32, view: &View) -> Result<()> {
        let data = self.seal(("views", id, message), view)?;
        sqlx::query(
            "INSERT INTO views (user_id, message_id, data) VALUES (?, ?, ?) \
             ON CONFLICT(user_id, message_id) DO UPDATE SET data = excluded.data, updated_at = unixepoch()",
        )
        .bind(id)
        .bind(message)
        .bind(data)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn view(&self, id: i64, message: i32) -> Result<Option<View>> {
        let data: Option<String> =
            sqlx::query_scalar("SELECT data FROM views WHERE user_id = ? AND message_id = ?")
                .bind(id)
                .bind(message)
                .fetch_optional(&self.pool)
                .await?;
        data.map(|data| self.unseal(("views", id, message), &data))
            .transpose()
    }

    /// Views older than 30 days and weeks older than 90 days are not worth keeping.
    pub async fn prune(&self) -> Result<()> {
        sqlx::query("DELETE FROM views WHERE updated_at < unixepoch() - 2592000")
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM schedules WHERE updated_at < unixepoch() - 7776000")
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
