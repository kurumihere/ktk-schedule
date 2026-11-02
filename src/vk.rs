use crate::{
    app::{App, login_required},
    model::*,
    render,
    storage::Account,
};
use anyhow::{Result, bail, ensure};
use chrono::NaiveDate;
use chrono_tz::Tz;
use futures::{StreamExt, future::BoxFuture, stream};
use reqwest::Url;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{
    sync::{
        Arc,
        atomic::{AtomicI32, Ordering},
    },
    time::Duration,
};
use tokio::{sync::Semaphore, task::JoinSet, time::Instant};
use tokio_util::sync::CancellationToken;

const API_VERSION: &str = "5.199";
/// A community token may call the API 20 times per second; keep a margin.
const CALL_SPACING: Duration = Duration::from_millis(60);
/// Updates handled at once; the long poll waits while all slots are busy.
const HANDLERS: usize = 16;
const HANDLER_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_BACKOFF: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VkKeyboard {
    pub inline: bool,
    pub buttons: Vec<Vec<VkButton>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VkButton {
    pub action: VkAction,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VkAction {
    Callback {
        label: String,
        payload: String,
    },
    Text {
        label: String,
        payload: Option<String>,
    },
    OpenLink {
        label: String,
        link: String,
    },
}

/// Long poll access. The key reads the community's events: never log it.
#[derive(Deserialize)]
pub struct LongPollServer {
    pub key: String,
    pub server: String,
    pub ts: String,
}

#[derive(Deserialize, Debug)]
pub struct LongPollResponse {
    #[serde(default)]
    pub ts: Option<String>,
    #[serde(default)]
    pub updates: Vec<LongPollUpdate>,
    #[serde(default)]
    pub failed: Option<u32>,
}

#[derive(Deserialize, Debug)]
pub struct LongPollUpdate {
    #[serde(rename = "type")]
    pub event_type: String,
    pub object: Value,
    #[serde(default)]
    pub group_id: i64,
}

#[derive(Deserialize, Debug)]
pub struct MessageNewObject {
    pub message: MessageObject,
}

#[derive(Deserialize)]
pub struct MessageObject {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub date: i64,
    pub peer_id: i64,
    pub from_id: i64,
    pub text: String,
    #[serde(default)]
    pub conversation_message_id: i64,
    #[serde(default)]
    pub payload: Option<String>,
    #[serde(default)]
    pub reply_message: Option<Box<MessageObject>>,
}

/// Message text may be `/login` with a password: keep it out of logs.
impl std::fmt::Debug for MessageObject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MessageObject")
            .field("id", &self.id)
            .field("peer_id", &self.peer_id)
            .field("from_id", &self.from_id)
            .field("text", &"<redacted>")
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize, Debug)]
pub struct MessageEventObject {
    pub user_id: i64,
    pub peer_id: i64,
    pub event_id: String,
    #[serde(default)]
    pub conversation_message_id: i64,
    #[serde(default)]
    pub payload: Value,
}

#[derive(Deserialize)]
struct VkApiEnvelope<T> {
    response: Option<T>,
    error: Option<VkApiError>,
}

#[derive(Deserialize, Debug)]
struct VkApiError {
    error_code: i64,
    error_msg: String,
}

/// An error VK reported for a method call.
#[derive(Debug)]
pub struct VkError {
    pub method: &'static str,
    pub code: i64,
    pub message: String,
}

impl std::fmt::Display for VkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "VK {} error {}: {}",
            self.method, self.code, self.message
        )
    }
}

impl std::error::Error for VkError {}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
enum SendMessageResult {
    Id(i64),
    Items(Vec<SendMessageItem>),
}

#[derive(Deserialize, Debug)]
struct SendMessageItem {
    #[serde(default)]
    message_id: i64,
    #[serde(default)]
    conversation_message_id: i64,
}

pub struct VkClient {
    http: reqwest::Client,
    token: String,
    group_id: i64,
    api: Url,
    /// Earliest moment the next API call may start.
    next_call: tokio::sync::Mutex<Instant>,
    random_id: AtomicI32,
}

impl VkClient {
    /// `api` is the HTTPS method base, such as `https://api.vk.com/method/`.
    pub fn new(token: String, group_id: i64, api: Url) -> Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(35))
            .build()?;
        // random_id deduplicates sends per peer: unique per process, distinct across restarts.
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i32 & 0x3fff_ffff)
            .unwrap_or(1);
        Ok(Self {
            http,
            token,
            group_id,
            api,
            next_call: tokio::sync::Mutex::new(Instant::now()),
            random_id: AtomicI32::new(seed),
        })
    }

    pub fn group_id(&self) -> i64 {
        self.group_id
    }

    async fn throttle(&self) {
        let wait = {
            let mut next = self.next_call.lock().await;
            let now = Instant::now();
            let slot = (*next).max(now);
            *next = slot + CALL_SPACING;
            slot - now
        };
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }

    /// Call an API method. Rate limiting (6) and internal errors (10) get one calm retry.
    async fn call<T: DeserializeOwned>(
        &self,
        method: &'static str,
        mut params: Vec<(&'static str, String)>,
    ) -> Result<T> {
        let url = self.api.join(method)?;
        params.push(("v", API_VERSION.into()));
        for attempt in 0..2 {
            self.throttle().await;
            let envelope: VkApiEnvelope<T> = self
                .http
                .post(url.clone())
                .bearer_auth(&self.token)
                .form(&params)
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .map_err(|e| e.without_url())?
                .json()
                .await
                .map_err(|e| e.without_url())?;
            match (envelope.response, envelope.error) {
                (_, Some(error)) if attempt == 0 && matches!(error.error_code, 6 | 10) => {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                (_, Some(error)) => {
                    return Err(VkError {
                        method,
                        code: error.error_code,
                        message: error.error_msg,
                    }
                    .into());
                }
                (Some(response), None) => return Ok(response),
                (None, None) => bail!("VK {method}: empty response"),
            }
        }
        bail!("VK {method}: still rate limited")
    }

    pub async fn get_long_poll_server(&self) -> Result<LongPollServer> {
        self.call(
            "groups.getLongPollServer",
            vec![("group_id", self.group_id.to_string())],
        )
        .await
    }

    pub async fn poll(&self, server: &LongPollServer, wait: u32) -> Result<LongPollResponse> {
        let mut url = Url::parse(&server.server)
            .or_else(|_| Url::parse(&format!("https://{}", server.server)))?;
        ensure!(
            url.scheme() == "https",
            "VK long poll server must use HTTPS"
        );
        url.query_pairs_mut()
            .append_pair("act", "a_check")
            .append_pair("key", &server.key)
            .append_pair("ts", &server.ts)
            .append_pair("wait", &wait.to_string());
        // Errors carry no URL: it contains the long poll key.
        Ok(self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| e.without_url())?
            .json()
            .await
            .map_err(|e| e.without_url())?)
    }

    fn next_random_id(&self) -> String {
        (self.random_id.fetch_add(1, Ordering::Relaxed) & 0x7fff_ffff).to_string()
    }

    pub async fn send_message(
        &self,
        peer_id: i64,
        text: &str,
        keyboard: Option<&VkKeyboard>,
    ) -> Result<i64> {
        let mut params = vec![
            ("peer_ids", peer_id.to_string()),
            ("random_id", self.next_random_id()),
            ("message", render::fit_text(text, 4096)),
        ];
        if let Some(keyboard) = keyboard {
            params.push(("keyboard", serde_json::to_string(keyboard)?));
        }
        Ok(
            match self
                .call::<SendMessageResult>("messages.send", params)
                .await?
            {
                SendMessageResult::Items(items) => items
                    .first()
                    .map(|item| {
                        if item.conversation_message_id > 0 {
                            item.conversation_message_id
                        } else {
                            item.message_id
                        }
                    })
                    .unwrap_or(0),
                SendMessageResult::Id(id) => id,
            },
        )
    }

    pub async fn edit_message(
        &self,
        peer_id: i64,
        conversation_message_id: i64,
        text: &str,
        keyboard: Option<&VkKeyboard>,
    ) -> Result<()> {
        let mut params = vec![
            ("peer_id", peer_id.to_string()),
            (
                "conversation_message_id",
                conversation_message_id.to_string(),
            ),
            ("message", render::fit_text(text, 4096)),
        ];
        if let Some(keyboard) = keyboard {
            params.push(("keyboard", serde_json::to_string(keyboard)?));
        }
        match self.call::<Value>("messages.edit", params).await {
            // Nothing changed.
            Err(error)
                if error
                    .downcast_ref::<VkError>()
                    .is_some_and(|e| e.code == 924) =>
            {
                Ok(())
            }
            result => result.map(|_| ()),
        }
    }

    pub async fn send_event_answer(
        &self,
        event_id: &str,
        user_id: i64,
        peer_id: i64,
        event_data: Option<&str>,
    ) -> Result<()> {
        let mut params = vec![
            ("event_id", event_id.to_string()),
            ("user_id", user_id.to_string()),
            ("peer_id", peer_id.to_string()),
        ];
        if let Some(data) = event_data {
            params.push(("event_data", data.to_string()));
        }
        self.call::<Value>("messages.sendMessageEventAnswer", params)
            .await
            .map(|_| ())
    }
}

pub fn button_cb(label: &str, data: &str, color: &str) -> VkButton {
    VkButton {
        action: VkAction::Callback {
            label: label.to_string(),
            payload: serde_json::json!({ "data": data }).to_string(),
        },
        color: Some(color.to_string()),
    }
}

pub fn schedule_keyboard_vk(
    days: &[ScheduleDay],
    view: &View,
    today: NaiveDate,
    tz: Tz,
) -> VkKeyboard {
    let mut buttons = Vec::new();

    // Row 1: Day prev / (Week select or Today) / Day next
    let is_today = view.date == today;
    let center_button = if is_today {
        button_cb(
            &format!("📅 {}", render::short_week_label(view.week())),
            "schedule:week:select",
            "primary",
        )
    } else {
        button_cb("📍 Сегодня", "schedule:today", "positive")
    };
    buttons.push(vec![
        button_cb("⬅️", "schedule:prev", "secondary"),
        center_button,
        button_cb("➡️", "schedule:next", "secondary"),
    ]);

    // Rows 2 & 3: Days direct selection (up to 6 days)
    let mut day_buttons = Vec::new();
    for (i, day) in days.iter().take(6).enumerate() {
        let selected = day_date(&day.date, tz) == Some(view.date);
        let color = if selected { "positive" } else { "secondary" };
        let prefix = if selected { "✅ " } else { "" };
        day_buttons.push(button_cb(
            &format!("{prefix}{}", render::short_day(day, tz)),
            &format!("schedule:day:{i}"),
            color,
        ));
    }
    if !day_buttons.is_empty() {
        if day_buttons.len() <= 3 {
            buttons.push(day_buttons);
        } else {
            let row1 = day_buttons.drain(..3).collect();
            buttons.push(row1);
            buttons.push(day_buttons);
        }
    }

    // Row 4: Sub-menu / Actions (strictly 1 button to keep total <= 10)
    if view.group.is_some() {
        buttons.push(vec![button_cb("🏠 Своя группа", "schedule:my", "positive")]);
    } else {
        buttons.push(vec![button_cb(
            "Меню",
            "schedule:subgroup:menu",
            "secondary",
        )]);
    }

    VkKeyboard {
        inline: true,
        buttons,
    }
}

pub fn week_keyboard_vk(view: &View, tz: Tz) -> Result<VkKeyboard> {
    let center = shift(view.week(), i64::from(view.week_offset) * 7)?;
    let mut buttons = vec![vec![
        button_cb("⬅️", "schedule:week:page:-1", "secondary"),
        button_cb("Назад", "schedule:back", "secondary"),
        button_cb("➡️", "schedule:week:page:1", "secondary"),
    ]];

    // 4 weeks around center to fit strictly within 6 rows limit:
    for delta in -1..=2 {
        let date = shift(center, delta * 7)?;
        let selected = date == view.week();
        let color = if selected { "positive" } else { "secondary" };
        let prefix = if selected { "✅ " } else { "" };
        buttons.push(vec![button_cb(
            &format!("{prefix}{}", render::short_week_label(date)),
            &format!("schedule:week:open:{}", week_millis(date, tz)?),
            color,
        )]);
    }

    buttons.push(vec![button_cb(
        "Текущая неделя",
        "schedule:week:today",
        "primary",
    )]);

    Ok(VkKeyboard {
        inline: true,
        buttons,
    })
}

pub fn subgroup_keyboard_vk(view: &View, teacher: bool) -> VkKeyboard {
    let mut buttons = Vec::new();

    if !teacher {
        let sub1_active = !view.show_all && view.subgroup == "left";
        let sub2_active = !view.show_all && view.subgroup == "right";
        let all_active = view.show_all;

        buttons.push(vec![
            button_cb(
                if sub1_active {
                    "✅ 1 подгруппа"
                } else {
                    "1 подгруппа"
                },
                "schedule:subgroup:set:left",
                if sub1_active { "positive" } else { "secondary" },
            ),
            button_cb(
                if sub2_active {
                    "✅ 2 подгруппа"
                } else {
                    "2 подгруппа"
                },
                "schedule:subgroup:set:right",
                if sub2_active { "positive" } else { "secondary" },
            ),
        ]);
        buttons.push(vec![button_cb(
            if all_active {
                "✅ Обе подгруппы"
            } else {
                "Обе подгруппы"
            },
            "schedule:subgroup:set:all",
            if all_active { "positive" } else { "secondary" },
        )]);
    }

    buttons.push(vec![button_cb(
        "📅 Выбрать неделю",
        "schedule:week:select",
        "secondary",
    )]);
    buttons.push(vec![button_cb(
        "🔍 Расписание другой группы",
        "schedule:group:select",
        "secondary",
    )]);
    buttons.push(vec![button_cb(
        "🔄 Обновить расписание",
        "schedule:refresh",
        "secondary",
    )]);
    buttons.push(vec![button_cb(
        "↩️ Назад к расписанию",
        "schedule:back",
        "primary",
    )]);

    VkKeyboard {
        inline: true,
        buttons,
    }
}

pub fn extract_callback_data(value: &Value) -> Option<String> {
    if let Some(obj) = value.as_object()
        && let Some(data) = obj.get("data").and_then(|v| v.as_str())
    {
        return Some(data.to_string());
    }
    if let Some(s) = value.as_str() {
        if let Ok(val) = serde_json::from_str::<Value>(s) {
            return extract_callback_data(&val);
        }
        return Some(s.to_string());
    }
    None
}

pub async fn handle_message(app: &Arc<App>, vk: &Arc<VkClient>, msg: MessageObject) -> Result<()> {
    // 1. Private chat first (AGENTS.md Golden Rule 1)
    if msg.peer_id != msg.from_id {
        let _ = vk
            .send_message(msg.peer_id, render::VK_PRIVATE_ONLY, None)
            .await;
        return Ok(());
    }

    let user_id = msg.from_id;
    let storage_id = PlatformUser::Vk(user_id).to_storage_id();
    let text = msg.text.trim();
    let (command, args) = crate::telegram::command(text);
    // VK buttons and people type words like "Начать": compare them case-insensitively.
    let word = text.to_lowercase();

    // 2. Rate limiting
    if !app.allow(storage_id, command == "/login").await {
        let _ = vk
            .send_message(msg.peer_id, render::TOO_MANY_REQUESTS, None)
            .await;
        return Ok(());
    }

    let _guard = app.lock_user(storage_id).await;

    // Handle commands
    if command == "/start" || word == "начать" {
        vk.send_message(msg.peer_id, render::VK_HELP, None).await?;
        return Ok(());
    }

    if command == "/login" {
        let credentials: Vec<&str> = args.split_whitespace().collect();
        let [login, password] = credentials[..] else {
            vk.send_message(msg.peer_id, render::LOGIN_USAGE, None)
                .await?;
            return Ok(());
        };
        match app.sign_in(storage_id, login, password).await {
            Ok(user_account) => {
                vk.send_message(msg.peer_id, &render::vk_signed_in(&user_account), None)
                    .await?;

                let view = View::own(&user_account, app.now().date_naive());
                show_vk(app, vk, &user_account, &view, None, false).await?;
            }
            Err(e) => {
                tracing::warn!(error=%e, "vk login failed");
                vk.send_message(msg.peer_id, crate::telegram::login_failure(&e), None)
                    .await?;
            }
        }
        return Ok(());
    }

    if command == "/logout" {
        app.sign_out(storage_id).await?;
        vk.send_message(msg.peer_id, render::LOGGED_OUT, None)
            .await?;
        return Ok(());
    }

    if command == "/my_id" {
        vk.send_message(msg.peer_id, &format!("VK ID: {}", msg.from_id), None)
            .await?;
        return Ok(());
    }

    if command == "/stats" {
        if app.config.owner_vk_id == 0 || msg.from_id != app.config.owner_vk_id {
            vk.send_message(msg.peer_id, render::NO_ACCESS, None)
                .await?;
            return Ok(());
        }
        vk.send_message(msg.peer_id, &app.stats().await?, None)
            .await?;
        return Ok(());
    }

    if command == "/announce" {
        announce_vk(app, vk, &msg, args).await?;
        return Ok(());
    }

    let Some(account) = app.storage.account(storage_id).await? else {
        vk.send_message(msg.peer_id, render::LOGIN_FIRST, None)
            .await?;
        return Ok(());
    };

    if command == "/notify_on" {
        app.storage.set_notify(storage_id, true).await?;
        vk.send_message(msg.peer_id, render::NOTIFY_ON, None)
            .await?;
        return Ok(());
    }

    if command == "/notify_off" {
        app.storage.set_notify(storage_id, false).await?;
        vk.send_message(msg.peer_id, render::NOTIFY_OFF, None)
            .await?;
        return Ok(());
    }

    if command == "/refresh" || word == "обновить" {
        let view = View::own(&account, app.now().date_naive());
        show_vk(app, vk, &account, &view, None, true).await?;
        return Ok(());
    }

    if command == "/my" || word == "моё" || word == "мое" {
        let view = View::own(&account, app.now().date_naive());
        show_vk(app, vk, &account, &view, None, false).await?;
        return Ok(());
    }

    if command == "/schedule" {
        let Ok(target_date) = parse_date(args, app.now().date_naive()) else {
            vk.send_message(msg.peer_id, render::DATE_FORMAT, None)
                .await?;
            return Ok(());
        };
        let view = View::own(&account, target_date);
        show_vk(app, vk, &account, &view, None, false).await?;
        return Ok(());
    }

    // Check if the user is inputting a group number
    if let Ok(group_id) = text.parse::<i64>()
        && (1..=100_000).contains(&group_id)
    {
        let mut view = View::own(&account, app.now().date_naive());
        view.group = Some(group_id);
        view.screen = Screen::Schedule;
        show_vk(app, vk, &account, &view, None, false).await?;
        return Ok(());
    }

    // Default response: show help
    vk.send_message(msg.peer_id, render::VK_HELP, None).await?;
    Ok(())
}

async fn announce_vk(app: &App, vk: &VkClient, msg: &MessageObject, param: &str) -> Result<()> {
    let peer_id = msg.peer_id;
    if app.config.owner_vk_id == 0 {
        vk.send_message(peer_id, render::VK_ANNOUNCE_OFF, None)
            .await?;
        return Ok(());
    }
    if msg.from_id != app.config.owner_vk_id {
        vk.send_message(peer_id, render::NO_ACCESS, None).await?;
        return Ok(());
    }
    let text = if !param.is_empty() {
        param
    } else if let Some(ref reply) = msg.reply_message {
        reply.text.trim()
    } else {
        ""
    };
    if text.is_empty() {
        vk.send_message(peer_id, render::ANNOUNCE_USAGE, None)
            .await?;
        return Ok(());
    }
    let all_ids = app.storage.ids().await?;
    let vk_recipients: Vec<i64> = all_ids
        .into_iter()
        .filter(|&id| id >= VK_ID_OFFSET)
        .map(|id| id - VK_ID_OFFSET)
        .collect();

    if vk_recipients.is_empty() {
        vk.send_message(peer_id, render::NO_RECIPIENTS, None)
            .await?;
        return Ok(());
    }

    // VkClient paces the calls; parallelism only hides network latency.
    let results =
        stream::iter(vk_recipients.into_iter().map(|recipient| async move {
            vk.send_message(recipient, text, None).await.map(|_| ())
        }))
        .boxed()
        .buffer_unordered(4)
        .collect::<Vec<_>>()
        .await;

    let sent = results.iter().filter(|r| r.is_ok()).count();
    vk.send_message(
        peer_id,
        &render::announce_done(sent, results.len() - sent),
        None,
    )
    .await?;
    Ok(())
}

pub async fn handle_callback(
    app: &Arc<App>,
    vk: &Arc<VkClient>,
    event: MessageEventObject,
) -> Result<()> {
    // 1. Private chat first
    if event.peer_id != event.user_id {
        let _ = vk
            .send_event_answer(
                &event.event_id,
                event.user_id,
                event.peer_id,
                Some(render::VK_PRIVATE_BUTTONS),
            )
            .await;
        return Ok(());
    }

    // Acknowledge the event
    let _ = vk
        .send_event_answer(&event.event_id, event.user_id, event.peer_id, None)
        .await;

    let Some(data) = extract_callback_data(&event.payload).filter(|d| d.starts_with("schedule:"))
    else {
        return Ok(());
    };
    let storage_id = PlatformUser::Vk(event.user_id).to_storage_id();
    // Buttons are as expensive as commands: refresh hits the college site.
    if !app.allow(storage_id, false).await {
        return Ok(());
    }
    let Ok(cmid) = i32::try_from(event.conversation_message_id) else {
        return Ok(());
    };
    let _guard = app.lock_user(storage_id).await;

    let Some(account) = app.storage.account(storage_id).await? else {
        return Ok(());
    };
    let teacher = !account.teacher_hash.is_empty();

    let mut view = match app.storage.view(storage_id, cmid).await? {
        Some(v) => v,
        None => View::own(&account, app.now().date_naive()),
    };

    let today = app.now().date_naive();
    let mut refresh = false;

    match data.as_str() {
        "schedule:prev" => {
            view.select_date(shift(view.date, -1)?);
            view.screen = Screen::Schedule;
        }
        "schedule:next" => {
            view.select_date(shift(view.date, 1)?);
            view.screen = Screen::Schedule;
        }
        "schedule:today" => {
            view.select_date(today);
            view.screen = Screen::Schedule;
        }
        "schedule:refresh" => {
            refresh = true;
            view.screen = Screen::Schedule;
        }
        "schedule:my" => {
            view = View::own(&account, today);
        }
        "schedule:back" => {
            view.screen = Screen::Schedule;
        }
        "schedule:group:select" => {
            view.screen = Screen::GroupInput;
            let keyboard = VkKeyboard {
                inline: true,
                buttons: vec![vec![button_cb("Назад", "schedule:back", "secondary")]],
            };
            vk.edit_message(
                event.peer_id,
                i64::from(cmid),
                render::GROUP_PROMPT,
                Some(&keyboard),
            )
            .await?;
            app.storage.save_view(storage_id, cmid, &view).await?;
            return Ok(());
        }
        "schedule:week:select" => {
            view.week_offset = 0;
            view.screen = Screen::Weeks;
        }
        "schedule:week:page:-1" | "schedule:week:page:1" => {
            let delta = if data.ends_with(":-1") { -4 } else { 4 };
            view.week_offset = (view.week_offset + delta).clamp(-520, 520);
            view.screen = Screen::Weeks;
        }
        "schedule:week:prev" => {
            view.select_date(shift(view.date, -7)?);
            view.screen = Screen::Schedule;
        }
        "schedule:week:next" => {
            view.select_date(shift(view.date, 7)?);
            view.screen = Screen::Schedule;
        }
        "schedule:week:today" => {
            view.week_offset = 0;
            view.select_date(today);
            view.screen = Screen::Schedule;
        }
        "schedule:subgroup:menu" => {
            view.screen = Screen::Subgroup;
        }
        "schedule:subgroup:set:all"
        | "schedule:subgroup:all"
        | "schedule:subgroup:set:left"
        | "schedule:subgroup:left"
        | "schedule:subgroup:set:right"
        | "schedule:subgroup:right" => {
            if teacher {
                return Ok(());
            }
            view.show_all = data.ends_with(":all");
            if !view.show_all {
                view.subgroup = if data.ends_with(":right") {
                    "right"
                } else {
                    "left"
                }
                .into();
            }
            // Filtering another group's timetable must not change the user's own preference.
            if view.group.is_none() {
                app.storage
                    .set_subgroup(storage_id, &view.subgroup, view.show_all)
                    .await?;
            }
            view.screen = Screen::Schedule;
        }
        _ if data.starts_with("schedule:day:") => {
            let Some(index) = data
                .strip_prefix("schedule:day:")
                .and_then(|i| i.parse::<usize>().ok())
            else {
                return Ok(());
            };
            let loaded = app.load(&account, &view, false).await?;
            if let Some(day) = loaded.days.get(index)
                && let Some(d) = day_date(&day.date, app.config.timezone)
            {
                view.select_date(d);
                view.screen = Screen::Schedule;
            }
        }
        _ if data.starts_with("schedule:week:open:") => {
            let Some(date) = data
                .strip_prefix("schedule:week:open:")
                .and_then(|millis| millis.parse().ok())
                .and_then(chrono::DateTime::from_timestamp_millis)
                .map(|at| at.with_timezone(&app.config.timezone).date_naive())
                .filter(|date| within_reach(*date, today))
            else {
                return Ok(());
            };
            view.select_date(week_start(date));
            view.week_offset = 0;
            view.screen = Screen::Schedule;
        }
        "schedule:download" => {
            // Note: download files for VK
            vk.send_message(event.peer_id, render::VK_FILES, None)
                .await?;
            return Ok(());
        }
        _ => return Ok(()),
    }

    show_vk(app, vk, &account, &view, Some(cmid), refresh).await
}

pub async fn show_vk(
    app: &App,
    vk: &VkClient,
    account: &Account,
    view: &View,
    cmid: Option<i32>,
    refresh: bool,
) -> Result<()> {
    let teacher = !account.teacher_hash.is_empty();
    match view.screen {
        Screen::Weeks => {
            let text = render::week_label(view.week());
            let keyboard = week_keyboard_vk(view, app.config.timezone)?;
            present_vk(app, vk, account.id, cmid, &text, keyboard, view).await
        }
        Screen::GroupInput => {
            let keyboard = VkKeyboard {
                inline: true,
                buttons: vec![vec![button_cb("Назад", "schedule:back", "secondary")]],
            };
            present_vk(
                app,
                vk,
                account.id,
                cmid,
                render::GROUP_PROMPT,
                keyboard,
                view,
            )
            .await
        }
        Screen::Subgroup => {
            let group = view.group.unwrap_or(account.group_id);
            let text = render::vk_menu(view, group, teacher);
            let keyboard = subgroup_keyboard_vk(view, teacher);
            present_vk(app, vk, account.id, cmid, &text, keyboard, view).await
        }
        Screen::Schedule => {
            // Only the timetable needs the site; menus work while it is down.
            let (view, loaded) = app.resolve(account, view, refresh).await?;
            let (text, _) = app
                .render_schedule_content(account, &view, &loaded, refresh, "")
                .await?;
            let keyboard = schedule_keyboard_vk(
                &loaded.days,
                &view,
                app.now().date_naive(),
                app.config.timezone,
            );
            present_vk(app, vk, account.id, cmid, &text, keyboard, &view).await
        }
    }
}

pub async fn present_vk(
    app: &App,
    vk: &VkClient,
    storage_id: i64,
    cmid: Option<i32>,
    text: &str,
    keyboard: VkKeyboard,
    view: &View,
) -> Result<()> {
    let PlatformUser::Vk(peer_id) = PlatformUser::from_storage_id(storage_id) else {
        bail!("present_vk called for a Telegram user");
    };

    let sent = |vk_id: i64| i32::try_from(vk_id).unwrap_or(0);
    let result_cmid = if let Some(id) = cmid {
        match vk
            .edit_message(peer_id, i64::from(id), text, Some(&keyboard))
            .await
        {
            Ok(()) => id,
            Err(e) => {
                tracing::warn!(error=%e, "failed to edit vk message, sending new");
                sent(vk.send_message(peer_id, text, Some(&keyboard)).await?)
            }
        }
    } else {
        sent(vk.send_message(peer_id, text, Some(&keyboard)).await?)
    };

    if result_cmid > 0 {
        app.storage.save_view(storage_id, result_cmid, view).await?;
    }
    Ok(())
}

/// Handle one update with a deadline; failures get an answer instead of silence.
async fn dispatch(app: Arc<App>, vk: Arc<VkClient>, update: LongPollUpdate) {
    let (peer, work): (i64, BoxFuture<'_, Result<()>>) = match update.event_type.as_str() {
        "message_new" => {
            let Ok(new) = serde_json::from_value::<MessageNewObject>(update.object) else {
                return;
            };
            (
                new.message.peer_id,
                Box::pin(handle_message(&app, &vk, new.message)),
            )
        }
        "message_event" => {
            let Ok(event) = serde_json::from_value::<MessageEventObject>(update.object) else {
                return;
            };
            (event.peer_id, Box::pin(handle_callback(&app, &vk, event)))
        }
        _ => return,
    };
    tokio::select! {
        _ = app.cancel.cancelled() => (),
        result = tokio::time::timeout(HANDLER_TIMEOUT, work) => match result {
            Ok(Ok(())) => (),
            Ok(Err(error)) => {
                if !login_required(&error) {
                    tracing::warn!(%error, "vk handler failed");
                }
                let _ = vk.send_message(peer, crate::telegram::failure(&error), None).await;
            }
            Err(_) => {
                let _ = vk.send_message(peer, render::TIMEOUT, None).await;
            }
        }
    }
}

/// Sleep for `delay` unless `stop` fires first; returns whether to stop.
async fn pause(stop: &CancellationToken, delay: Duration) -> bool {
    tokio::select! {
        _ = stop.cancelled() => true,
        _ = tokio::time::sleep(delay) => false,
    }
}

/// VK Bots Long Poll until `stop`; then waits for the handlers still running.
pub async fn run(app: Arc<App>, vk: Arc<VkClient>, stop: CancellationToken) {
    tracing::info!(group_id = vk.group_id(), "vk bot loop started");
    let slots = Arc::new(Semaphore::new(HANDLERS));
    let mut handlers = JoinSet::new();
    let mut server: Option<LongPollServer> = None;
    // After an expired key, continue from the last event seen instead of skipping ahead.
    let mut resume: Option<String> = None;
    let mut backoff = Duration::from_secs(2);
    while !stop.is_cancelled() {
        while handlers.try_join_next().is_some() {}
        if server.is_none() {
            let fetched = tokio::select! {
                _ = stop.cancelled() => break,
                fetched = vk.get_long_poll_server() => fetched,
            };
            match fetched {
                Ok(mut fresh) => {
                    if let Some(ts) = resume.take() {
                        fresh.ts = ts;
                    }
                    backoff = Duration::from_secs(2);
                    server = Some(fresh);
                }
                Err(error) => {
                    tracing::error!(%error, "could not get the vk long poll server; retrying in {backoff:?}");
                    if pause(&stop, backoff).await {
                        break;
                    }
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                    continue;
                }
            }
        }
        let Some(current) = server.as_mut() else {
            continue;
        };
        let polled = tokio::select! {
            _ = stop.cancelled() => break,
            polled = vk.poll(current, 25) => polled,
        };
        let response = match polled {
            Ok(response) => {
                backoff = Duration::from_secs(2);
                response
            }
            Err(error) => {
                tracing::warn!(%error, "vk long poll failed; retrying in {backoff:?}");
                if pause(&stop, backoff).await {
                    break;
                }
                backoff = (backoff * 2).min(MAX_BACKOFF);
                continue;
            }
        };
        match response.failed {
            None => (),
            // Events were lost; continue from the server's new position.
            Some(1) => {
                if let Some(ts) = response.ts {
                    current.ts = ts;
                }
                continue;
            }
            // The key expires routinely (about hourly): renew it, keep the position.
            Some(2) => {
                tracing::debug!("vk long poll key expired; renewing");
                resume = Some(current.ts.clone());
                server = None;
                continue;
            }
            Some(code) => {
                tracing::info!(code, "vk long poll session lost; starting over");
                server = None;
                continue;
            }
        }
        if let Some(ts) = response.ts {
            current.ts = ts;
        }
        for update in response.updates {
            let permit = tokio::select! {
                _ = stop.cancelled() => break,
                permit = slots.clone().acquire_owned() => permit,
            };
            let Ok(permit) = permit else { break };
            let (app, vk) = (app.clone(), vk.clone());
            handlers.spawn(async move {
                let _permit = permit;
                dispatch(app, vk, update).await;
            });
        }
    }
    // Requests already in progress finish; the caller bounds this wait.
    while handlers.join_next().await.is_some() {}
    tracing::info!("vk bot loop stopped");
}
