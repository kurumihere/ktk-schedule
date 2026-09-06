use crate::{
    credentials::Cipher,
    model::{ScheduleDay, View},
};
use anyhow::Result;
use sqlx::{
    Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use std::{path::Path, time::Duration};

#[derive(Clone)]
pub struct Account {
    pub id: i64,
    pub login: String,
    pub password: String,
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
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));
        // SQLite serializes writes. One asynchronous worker also avoids pool-level write contention.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        sqlx::migrate!().run(&pool).await?;
        Ok(Self { pool, cipher })
    }

    pub async fn close(&self) {
        self.pool.close().await;
    }

    pub async fn account(&self, id: i64) -> Result<Option<Account>> {
        let Some(row) = sqlx::query("SELECT * FROM accounts WHERE telegram_id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
        else {
            return Ok(None);
        };
        Ok(Some(Account {
            id,
            login: row.try_get("login")?,
            password: self.cipher.decrypt(id, row.try_get("password")?)?,
            group_id: row.try_get("group_id")?,
            personal_subgroup: row.try_get("personal_subgroup")?,
            subgroup: row.try_get("subgroup")?,
            show_all: row.try_get("show_all")?,
            teacher_hash: row.try_get("teacher_hash")?,
            notify: row.try_get("notify")?,
        }))
    }

    pub async fn save_account(&self, a: &Account) -> Result<()> {
        let password = self.cipher.encrypt(a.id, &a.password)?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("INSERT INTO accounts (telegram_id,login,password,group_id,personal_subgroup,subgroup,show_all,teacher_hash,notify) VALUES (?,?,?,?,?,?,?,?,?) ON CONFLICT(telegram_id) DO UPDATE SET login=excluded.login,password=excluded.password,group_id=excluded.group_id,personal_subgroup=excluded.personal_subgroup,subgroup=excluded.subgroup,show_all=excluded.show_all,teacher_hash=excluded.teacher_hash")
            .bind(a.id).bind(&a.login).bind(password).bind(a.group_id).bind(&a.personal_subgroup).bind(&a.subgroup).bind(a.show_all).bind(&a.teacher_hash).bind(a.notify).execute(&mut *tx).await?;
        // A Telegram user may sign in to a different workspace account.
        sqlx::query("DELETE FROM schedules WHERE telegram_id=?")
            .bind(a.id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM views WHERE telegram_id=?")
            .bind(a.id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn set_subgroup(&self, id: i64, subgroup: &str, all: bool) -> Result<()> {
        sqlx::query("UPDATE accounts SET subgroup=?,show_all=? WHERE telegram_id=?")
            .bind(subgroup)
            .bind(all)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn set_notify(&self, id: i64, enabled: bool) -> Result<()> {
        sqlx::query("UPDATE accounts SET notify=? WHERE telegram_id=?")
            .bind(enabled)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn ids(&self) -> Result<Vec<i64>> {
        Ok(
            sqlx::query_scalar("SELECT telegram_id FROM accounts ORDER BY telegram_id")
                .fetch_all(&self.pool)
                .await?,
        )
    }

    pub async fn notification_ids(&self, date: &str) -> Result<Vec<i64>> {
        Ok(sqlx::query_scalar("SELECT telegram_id FROM accounts WHERE notify=1 AND (notified_date IS NULL OR notified_date<>?) ORDER BY telegram_id").bind(date).fetch_all(&self.pool).await?)
    }

    pub async fn mark_notified(&self, id: i64, date: &str) -> Result<()> {
        sqlx::query("UPDATE accounts SET notified_date=? WHERE telegram_id=?")
            .bind(date)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn counts(&self) -> Result<(i64, i64)> {
        Ok(
            sqlx::query_as("SELECT COUNT(*), COALESCE(SUM(notify),0) FROM accounts")
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
        sqlx::query("INSERT INTO schedules(telegram_id,scope,week,data) VALUES (?,?,?,?) ON CONFLICT(telegram_id,scope,week) DO UPDATE SET data=excluded.data,updated_at=unixepoch()")
            .bind(id).bind(scope).bind(week).bind(serde_json::to_string(days)?).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn schedule(
        &self,
        id: i64,
        scope: &str,
        week: &str,
    ) -> Result<Option<Vec<ScheduleDay>>> {
        let data: Option<String> = sqlx::query_scalar(
            "SELECT data FROM schedules WHERE telegram_id=? AND scope=? AND week=?",
        )
        .bind(id)
        .bind(scope)
        .bind(week)
        .fetch_optional(&self.pool)
        .await?;
        Ok(data.map(|s| serde_json::from_str(&s)).transpose()?)
    }

    pub async fn save_view(&self, id: i64, message: i32, view: &View) -> Result<()> {
        sqlx::query("INSERT INTO views(telegram_id,message_id,data) VALUES (?,?,?) ON CONFLICT(telegram_id,message_id) DO UPDATE SET data=excluded.data,updated_at=unixepoch()")
            .bind(id).bind(message).bind(serde_json::to_string(view)?).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn view(&self, id: i64, message: i32) -> Result<Option<View>> {
        let data: Option<String> =
            sqlx::query_scalar("SELECT data FROM views WHERE telegram_id=? AND message_id=?")
                .bind(id)
                .bind(message)
                .fetch_optional(&self.pool)
                .await?;
        Ok(data.map(|s| serde_json::from_str(&s)).transpose()?)
    }

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
