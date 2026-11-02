use anyhow::{Context, Result, ensure};
use chrono::NaiveTime;
use chrono_tz::Tz;
use reqwest::Url;
use std::path::PathBuf;

/// Secrets may come from systemd credentials (`LoadCredentialEncrypted=`, file name in
/// lower case) instead of the environment; a credential wins over a variable.
const SECRETS: [&str; 3] = ["BOT_TOKEN", "CREDENTIALS_SECRET", "VK_GROUP_TOKEN"];

pub struct Config {
    pub token: String,
    pub secret: String,
    pub base_url: Url,
    pub database_path: String,
    pub device: String,
    pub default_group: i64,
    pub default_subgroup: String,
    pub owner_id: i64,
    pub owner_vk_id: i64,
    pub notify_time: NaiveTime,
    pub timezone: Tz,
    /// Set together with `vk_group_id`; VK is off without it.
    pub vk_group_token: Option<String>,
    pub vk_group_id: Option<i64>,
    /// HTTPS method base ending in `/`; the community token is sent there.
    pub vk_api_url: Url,
}

impl Config {
    pub fn load() -> Result<Self> {
        let credentials = std::env::var_os("CREDENTIALS_DIRECTORY").map(PathBuf::from);
        Self::from_values(|key| {
            credentials
                .as_ref()
                .filter(|_| SECRETS.contains(&key))
                .and_then(|dir| std::fs::read_to_string(dir.join(key.to_ascii_lowercase())).ok())
                .or_else(|| std::env::var(key).ok())
        })
    }

    pub fn from_values(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let value = |key: &str, fallback: &str| {
            get(key)
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| fallback.into())
        };
        let optional = |key: &str| {
            get(key)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
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
        let base_url = https_url(
            "KTK_BASE_URL",
            &value("KTK_BASE_URL", "https://ktk.education-system.ru/"),
        )?;
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
        let owner_vk_id = value("OWNER_VK_ID", "0")
            .parse()
            .context("invalid OWNER_VK_ID")?;
        ensure!(owner_vk_id >= 0, "OWNER_VK_ID cannot be negative");
        let vk_group_token = optional("VK_GROUP_TOKEN");
        let vk_group_id = optional("VK_GROUP_ID")
            .map(|id| id.parse::<i64>().context("invalid VK_GROUP_ID"))
            .transpose()?;
        ensure!(
            vk_group_id.is_none_or(|id| id > 0),
            "VK_GROUP_ID must be a positive community ID"
        );
        ensure!(
            vk_group_token.is_none() || vk_group_id.is_some(),
            "VK_GROUP_ID is required when VK_GROUP_TOKEN is set"
        );
        let mut vk_api_url = https_url(
            "VK_API_URL",
            &value("VK_API_URL", "https://api.vk.com/method/"),
        )?;
        // Methods are joined onto this URL: without the slash the last segment would be lost.
        if !vk_api_url.path().ends_with('/') {
            let path = format!("{}/", vk_api_url.path());
            vk_api_url.set_path(&path);
        }
        Ok(Self {
            token,
            secret,
            base_url,
            default_group,
            default_subgroup,
            owner_id,
            owner_vk_id,
            database_path: value("DATABASE_PATH", "data/bot.sqlite3"),
            device: value("KTK_DEVICE_NAME", "ktk-schedule"),
            notify_time: NaiveTime::parse_from_str(&value("NOTIFY_TIME", "07:30"), "%H:%M")
                .context("NOTIFY_TIME must be HH:MM")?,
            timezone: value("TIMEZONE", "Asia/Yekaterinburg")
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid TIMEZONE"))?,
            vk_group_token,
            vk_group_id,
            vk_api_url,
        })
    }
}

/// Credentials and tokens never travel over plain HTTP or inside the URL itself.
fn https_url(key: &str, raw: &str) -> Result<Url> {
    let url = Url::parse(raw.trim()).with_context(|| format!("invalid {key}"))?;
    ensure!(
        url.scheme() == "https" && url.host_str().is_some(),
        "{key} must be an HTTPS URL"
    );
    ensure!(
        url.username().is_empty() && url.password().is_none(),
        "{key} must not contain credentials"
    );
    Ok(url)
}
