use anyhow::{Context, Result, ensure};
use chrono::NaiveTime;
use chrono_tz::Tz;
use reqwest::Url;

pub struct Config {
    pub token: String,
    pub secret: String,
    pub base_url: Url,
    pub database_path: String,
    pub device: String,
    pub default_group: i64,
    pub default_subgroup: String,
    pub owner_id: i64,
    pub notify_time: NaiveTime,
    pub timezone: Tz,
}

impl Config {
    pub fn load() -> Result<Self> {
        Self::from_values(|key| std::env::var(key).ok())
    }

    pub fn from_values(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let value = |key: &str, fallback: &str| {
            get(key)
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| fallback.into())
        };
        let token = value("BOT_TOKEN", "").trim().to_owned();
        let (id, token_secret) = token
            .split_once(':')
            .context("BOT_TOKEN is required (get it from @BotFather)")?;
        ensure!(
            !id.is_empty()
                && id.bytes().all(|c| c.is_ascii_digit())
                && !token_secret.is_empty()
                && !token_secret.contains(char::is_whitespace),
            "invalid BOT_TOKEN format"
        );
        let secret = value("CREDENTIALS_SECRET", "").trim().to_owned();
        ensure!(
            secret.len() >= 32,
            "CREDENTIALS_SECRET must contain at least 32 bytes"
        );
        let base_url = Url::parse(&value("KTK_BASE_URL", "https://workspace.ktk-45.ru/"))?;
        ensure!(
            matches!(base_url.scheme(), "https" | "http") && base_url.host_str().is_some(),
            "KTK_BASE_URL must be an HTTP(S) URL"
        );
        ensure!(
            base_url.username().is_empty() && base_url.password().is_none(),
            "KTK_BASE_URL must not contain credentials"
        );
        let default_group = value("DEFAULT_GROUP_ID", "269")
            .parse()
            .context("invalid DEFAULT_GROUP_ID")?;
        ensure!(
            (1..=100_000).contains(&default_group),
            "DEFAULT_GROUP_ID must be between 1 and 100000"
        );
        let default_subgroup = crate::model::personal_subgroup(&value("DEFAULT_SUBGROUP", "1"))
            .context("invalid DEFAULT_SUBGROUP")?
            .into();
        let owner_id = value("OWNER_TELEGRAM_ID", "0")
            .parse()
            .context("invalid OWNER_TELEGRAM_ID")?;
        ensure!(owner_id >= 0, "OWNER_TELEGRAM_ID cannot be negative");
        Ok(Self {
            token,
            secret,
            base_url,
            default_group,
            default_subgroup,
            owner_id,
            database_path: value("DATABASE_PATH", "data/bot.sqlite3"),
            device: value("KTK_DEVICE_NAME", "ktk-schedule"),
            notify_time: NaiveTime::parse_from_str(&value("NOTIFY_TIME", "07:30"), "%H:%M")
                .context("NOTIFY_TIME must be HH:MM")?,
            timezone: value("TIMEZONE", "Asia/Yekaterinburg")
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid TIMEZONE"))?,
        })
    }
}
