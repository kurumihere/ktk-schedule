use crate::{app::App, model::*, render, storage::Account};
use anyhow::{Result, anyhow};
use chrono::{Days, NaiveDate};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

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

#[derive(Deserialize, Debug)]
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

#[derive(Deserialize, Debug)]
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

#[derive(Deserialize, Debug)]
struct VkApiEnvelope<T> {
    response: Option<T>,
    error: Option<VkApiError>,
}

#[derive(Deserialize, Debug)]
struct VkApiError {
    error_code: i64,
    error_msg: String,
}

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
    api_url: String,
}

impl VkClient {
    pub fn new(token: String, group_id: i64, api_url: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(35))
            .build()
            .unwrap_or_default();
        Self {
            http,
            token,
            group_id,
            api_url,
        }
    }

    pub fn group_id(&self) -> i64 {
        self.group_id
    }

    pub async fn get_long_poll_server(&self) -> Result<LongPollServer> {
        let url = format!(
            "{}/groups.getLongPollServer",
            self.api_url.trim_end_matches('/')
        );
        let res: VkApiEnvelope<LongPollServer> = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .form(&[
                ("group_id", self.group_id.to_string()),
                ("v", "5.199".to_string()),
            ])
            .send()
            .await?
            .json()
            .await?;
        if let Some(err) = res.error {
            return Err(anyhow!(
                "VK getLongPollServer error {}: {}",
                err.error_code,
                err.error_msg
            ));
        }
        res.response
            .ok_or_else(|| anyhow!("empty response from getLongPollServer"))
    }

    pub async fn poll(
        &self,
        server: &str,
        key: &str,
        ts: &str,
        wait: u32,
    ) -> Result<LongPollResponse> {
        let server_url = if server.starts_with("http") {
            server.to_string()
        } else {
            format!("https://{server}")
        };
        let sep = if server_url.contains('?') { '&' } else { '?' };
        let poll_url = format!("{server_url}{sep}act=a_check&key={key}&ts={ts}&wait={wait}");
        let res = self
            .http
            .get(&poll_url)
            .send()
            .await?
            .json::<LongPollResponse>()
            .await?;
        Ok(res)
    }

    pub async fn send_message(
        &self,
        peer_id: i64,
        text: &str,
        keyboard: Option<&VkKeyboard>,
    ) -> Result<i64> {
        let url = format!("{}/messages.send", self.api_url.trim_end_matches('/'));
        let random_id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| (d.as_nanos() & 0x7FFF_FFFF) as i32)
            .unwrap_or(1)
            .to_string();
        let mut form = vec![
            ("peer_id", peer_id.to_string()),
            ("random_id", random_id),
            ("message", render::fit_text(text, 4096)),
            ("v", "5.199".to_string()),
        ];
        let kb_json;
        if let Some(kb) = keyboard {
            kb_json = serde_json::to_string(kb)?;
            form.push(("keyboard", kb_json));
        }
        let res: VkApiEnvelope<SendMessageResult> = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .form(&form)
            .send()
            .await?
            .json()
            .await?;
        if let Some(err) = res.error {
            return Err(anyhow!(
                "VK messages.send error {}: {}",
                err.error_code,
                err.error_msg
            ));
        }
        match res.response {
            Some(SendMessageResult::Id(id)) => Ok(id),
            Some(SendMessageResult::Items(items)) if !items.is_empty() => {
                let cmid = items[0].conversation_message_id;
                if cmid > 0 {
                    Ok(cmid)
                } else {
                    Ok(items[0].message_id)
                }
            }
            _ => Ok(0),
        }
    }

    pub async fn edit_message(
        &self,
        peer_id: i64,
        conversation_message_id: i64,
        text: &str,
        keyboard: Option<&VkKeyboard>,
    ) -> Result<()> {
        let url = format!("{}/messages.edit", self.api_url.trim_end_matches('/'));
        let mut form = vec![
            ("peer_id", peer_id.to_string()),
            (
                "conversation_message_id",
                conversation_message_id.to_string(),
            ),
            ("message", render::fit_text(text, 4096)),
            ("v", "5.199".to_string()),
        ];
        let kb_json;
        if let Some(kb) = keyboard {
            kb_json = serde_json::to_string(kb)?;
            form.push(("keyboard", kb_json));
        }
        let res: VkApiEnvelope<Value> = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .form(&form)
            .send()
            .await?
            .json()
            .await?;
        if let Some(err) = res.error {
            if err.error_code == 924 {
                // Nothing changed
                return Ok(());
            }
            return Err(anyhow!(
                "VK messages.edit error {}: {}",
                err.error_code,
                err.error_msg
            ));
        }
        Ok(())
    }

    pub async fn send_event_answer(
        &self,
        event_id: &str,
        user_id: i64,
        peer_id: i64,
        event_data: Option<&str>,
    ) -> Result<()> {
        let url = format!(
            "{}/messages.sendMessageEventAnswer",
            self.api_url.trim_end_matches('/')
        );
        let mut form = vec![
            ("event_id", event_id.to_string()),
            ("user_id", user_id.to_string()),
            ("peer_id", peer_id.to_string()),
            ("v", "5.199".to_string()),
        ];
        if let Some(data) = event_data {
            form.push(("event_data", data.to_string()));
        }
        let res: VkApiEnvelope<Value> = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .form(&form)
            .send()
            .await?
            .json()
            .await?;
        if let Some(err) = res.error {
            return Err(anyhow!(
                "VK sendMessageEventAnswer error {}: {}",
                err.error_code,
                err.error_msg
            ));
        }
        Ok(())
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
    _today: NaiveDate,
    tz: Tz,
    _file_count: usize,
    teacher: bool,
) -> VkKeyboard {
    let mut buttons = Vec::new();

    // Row 1: Week navigation
    buttons.push(vec![
        button_cb("⬅️ неделя", "schedule:week:prev", "secondary"),
        button_cb(
            &render::week_label(view.week()),
            "schedule:week:select",
            "primary",
        ),
        button_cb("неделя ➡️", "schedule:week:next", "secondary"),
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
    } else if !teacher {
        let sub_label = if view.show_all {
            "👥 Подгруппа: Обе"
        } else if view.subgroup == "left" {
            "👤 Подгруппа: 1"
        } else {
            "👤 Подгруппа: 2"
        };
        buttons.push(vec![button_cb(
            sub_label,
            "schedule:subgroup:menu",
            "secondary",
        )]);
    } else {
        buttons.push(vec![button_cb(
            "🔍 Расписание группы",
            "schedule:group:select",
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
            &format!("{prefix}{}", render::week_label(date)),
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

pub fn subgroup_keyboard_vk(view: &View) -> VkKeyboard {
    let sub1_active = !view.show_all && view.subgroup == "left";
    let sub2_active = !view.show_all && view.subgroup == "right";
    let all_active = view.show_all;

    let buttons = vec![
        vec![button_cb(
            if sub1_active {
                "✅ 1 подгруппа"
            } else {
                "1 подгруппа"
            },
            "schedule:subgroup:set:left",
            if sub1_active { "positive" } else { "secondary" },
        )],
        vec![button_cb(
            if sub2_active {
                "✅ 2 подгруппа"
            } else {
                "2 подгруппа"
            },
            "schedule:subgroup:set:right",
            if sub2_active { "positive" } else { "secondary" },
        )],
        vec![button_cb(
            if all_active {
                "✅ Обе подгруппы"
            } else {
                "Обе подгруппы"
            },
            "schedule:subgroup:set:all",
            if all_active { "positive" } else { "secondary" },
        )],
        vec![button_cb(
            "🔍 Расписание другой группы",
            "schedule:group:select",
            "secondary",
        )],
        vec![button_cb(
            "🔄 Обновить расписание",
            "schedule:refresh",
            "secondary",
        )],
        vec![button_cb(
            "↩️ Назад к расписанию",
            "schedule:back",
            "primary",
        )],
    ];

    VkKeyboard {
        inline: true,
        buttons,
    }
}

pub fn shift(base: NaiveDate, days: i64) -> Result<NaiveDate> {
    if days >= 0 {
        base.checked_add_days(Days::new(days as u64))
    } else {
        base.checked_sub_days(Days::new((-days) as u64))
    }
    .ok_or_else(|| anyhow!("date overflow"))
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
            .send_message(
                msg.peer_id,
                "Расписание доступно только в личных сообщениях с сообществом.",
                None,
            )
            .await;
        return Ok(());
    }

    let user_id = msg.from_id;
    let storage_id = PlatformUser::Vk(user_id).to_storage_id();

    // 2. Rate limiting
    let text = msg.text.trim();
    let is_login = text.starts_with("/login");
    if !app.allow(storage_id, is_login).await {
        let _ = vk
            .send_message(
                msg.peer_id,
                "Слишком много запросов. Пожалуйста, подождите немного.",
                None,
            )
            .await;
        return Ok(());
    }

    let _guard = app.lock_user(storage_id).await;

    // Handle commands
    if text == "/start" || text.eq_ignore_ascii_case("начать") {
        vk.send_message(msg.peer_id, render::HELP, None).await?;
        return Ok(());
    }

    if is_login {
        let parts: Vec<&str> = text.split_whitespace().collect();
        if parts.len() != 3 {
            vk.send_message(
                msg.peer_id,
                "Использование: /login <логин> <пароль>\nПример: /login 1000123 mypass",
                None,
            )
            .await?;
            return Ok(());
        }
        let (login, password) = (parts[1], parts[2]);
        match app.sign_in(storage_id, login, password).await {
            Ok(user_account) => {
                let role = if !user_account.teacher_hash.is_empty() {
                    "преподаватель".to_string()
                } else {
                    format!("группа {}", user_account.group_id)
                };
                let welcome = format!(
                    "✅ Вы успешно вошли в систему ({role})!\n\n⚠️ Внимание: ВКонтакте не позволяет боту автоматически удалить ваше сообщение с паролем. Рекомендуем вручную удалить сообщение с логином и паролем из этого чата в целях безопасности."
                );
                vk.send_message(msg.peer_id, &welcome, None).await?;

                let view = View::own(&user_account, app.now().date_naive());
                show_vk(app, vk, &user_account, &view, None, false).await?;
            }
            Err(e) => {
                tracing::warn!(error=%e, "vk login failed");
                vk.send_message(
                    msg.peer_id,
                    "Неверный логин или пароль, либо сервис недоступен.",
                    None,
                )
                .await?;
            }
        }
        return Ok(());
    }

    if text == "/logout" {
        app.sign_out(storage_id).await?;
        vk.send_message(
            msg.peer_id,
            "Вы вышли из системы. Ваши сохранённые данные удалены.",
            None,
        )
        .await?;
        return Ok(());
    }

    let Some(account) = app.storage.account(storage_id).await? else {
        vk.send_message(
            msg.peer_id,
            "Вы не авторизованы. Для входа отправьте:\n/login логин пароль",
            None,
        )
        .await?;
        return Ok(());
    };

    if text == "/notify_on" {
        app.storage.set_notify(storage_id, true).await?;
        vk.send_message(msg.peer_id, "🔔 Утренние уведомления включены.", None)
            .await?;
        return Ok(());
    }

    if text == "/notify_off" {
        app.storage.set_notify(storage_id, false).await?;
        vk.send_message(msg.peer_id, "🔕 Утренние уведомления выключены.", None)
            .await?;
        return Ok(());
    }

    if text.eq_ignore_ascii_case("/refresh") || text.eq_ignore_ascii_case("обновить") {
        let view = View::own(&account, app.now().date_naive());
        show_vk(app, vk, &account, &view, None, true).await?;
        return Ok(());
    }

    if text.eq_ignore_ascii_case("/my")
        || text.eq_ignore_ascii_case("моё")
        || text.eq_ignore_ascii_case("мое")
    {
        let view = View::own(&account, app.now().date_naive());
        show_vk(app, vk, &account, &view, None, false).await?;
        return Ok(());
    }

    if text.starts_with("/schedule") {
        let arg = text.strip_prefix("/schedule").unwrap_or("").trim();
        let target_date = if arg.is_empty() {
            app.now().date_naive()
        } else {
            match parse_date(arg, app.now().date_naive()) {
                Ok(d) => d,
                Err(_) => {
                    vk.send_message(
                        msg.peer_id,
                        "Неверный формат даты. Используйте: /schedule ДД.ММ или /schedule ГГГГ-ММ-ДД",
                        None,
                    )
                    .await?;
                    return Ok(());
                }
            }
        };
        let mut view = View::own(&account, target_date);
        view.select_date(target_date);
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
    vk.send_message(msg.peer_id, render::HELP, None).await?;
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
                Some(r#"{"type":"show_snackbar","text":"Кнопки доступны только в личных сообщениях"}"#),
            )
            .await;
        return Ok(());
    }

    // Acknowledge the event
    let _ = vk
        .send_event_answer(&event.event_id, event.user_id, event.peer_id, None)
        .await;

    let Some(data) = extract_callback_data(&event.payload) else {
        return Ok(());
    };
    if !data.starts_with("schedule:") {
        return Ok(());
    }

    let storage_id = PlatformUser::Vk(event.user_id).to_storage_id();
    let _guard = app.lock_user(storage_id).await;

    let Some(account) = app.storage.account(storage_id).await? else {
        return Ok(());
    };

    let cmid = event.conversation_message_id as i32;
    let Some(mut view) = app.storage.view(storage_id, cmid).await? else {
        // Fallback to fresh view if expired
        let view = View::own(&account, app.now().date_naive());
        show_vk(app, vk, &account, &view, Some(cmid), false).await?;
        return Ok(());
    };

    let now = app.now();
    let today = now.date_naive();
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
                cmid as i64,
                "Введите номер группы сообщением в чат (например: 269):",
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
            view.week_start = shift(view.week(), -7)?;
            view.select_date(shift(view.date, -7)?);
            view.screen = Screen::Schedule;
        }
        "schedule:week:next" => {
            view.week_start = shift(view.week(), 7)?;
            view.select_date(shift(view.date, 7)?);
            view.screen = Screen::Schedule;
        }
        "schedule:week:today" => {
            view.week_offset = 0;
            view.week_start = week_start(today);
            view.select_date(today);
            view.screen = Screen::Schedule;
        }
        "schedule:subgroup:menu" => {
            view.screen = Screen::Subgroup;
        }
        "schedule:subgroup:set:all" | "schedule:subgroup:all" => {
            view.show_all = true;
            let _ = app
                .storage
                .set_subgroup(storage_id, &view.subgroup, true)
                .await;
            view.screen = Screen::Schedule;
        }
        "schedule:subgroup:set:left" | "schedule:subgroup:left" => {
            view.show_all = false;
            view.subgroup = "left".into();
            let _ = app.storage.set_subgroup(storage_id, "left", false).await;
            view.screen = Screen::Schedule;
        }
        "schedule:subgroup:set:right" | "schedule:subgroup:right" => {
            view.show_all = false;
            view.subgroup = "right".into();
            let _ = app.storage.set_subgroup(storage_id, "right", false).await;
            view.screen = Screen::Schedule;
        }
        _ if data.starts_with("schedule:day:") => {
            let idx = data
                .strip_prefix("schedule:day:")
                .unwrap_or("")
                .parse::<usize>()
                .unwrap_or(0);
            let loaded = app.load(&account, &view, false).await?;
            if let Some(day) = loaded.days.get(idx)
                && let Some(d) = day_date(&day.date, app.config.timezone)
            {
                view.select_date(d);
                view.screen = Screen::Schedule;
            }
        }
        _ if data.starts_with("schedule:week:open:") => {
            if let Ok(millis) = data
                .strip_prefix("schedule:week:open:")
                .unwrap_or("")
                .parse::<i64>()
            {
                let sec = millis / 1000;
                let nsec = ((millis % 1000) * 1_000_000) as u32;
                if let Some(dt) = chrono::DateTime::from_timestamp(sec, nsec) {
                    let d = dt.with_timezone(&app.config.timezone).date_naive();
                    view.week_start = week_start(d);
                    view.select_date(d);
                    view.screen = Screen::Schedule;
                }
            }
        }
        "schedule:download" => {
            // Note: download files for VK
            vk.send_message(
                event.peer_id,
                "Скачивание файлов в VK пока не поддерживается. Используйте веб-версию или Telegram-бота.",
                None,
            )
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
    let loaded = app.load(account, view, refresh).await?;
    let now = app.now();
    let teacher = !account.teacher_hash.is_empty();

    match view.screen {
        Screen::Weeks => {
            let text = format!(
                "Выбери неделю:\n\nСейчас открыта {}",
                render::week_label(view.week())
            );
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
                "Введите номер группы (например: 269):",
                keyboard,
                view,
            )
            .await
        }
        Screen::Subgroup => {
            let current = if view.show_all {
                "Обе подгруппы"
            } else if view.subgroup == "left" {
                "1 подгруппа"
            } else {
                "2 подгруппа"
            };
            let group_num = view.group.unwrap_or(account.group_id);
            let text = format!(
                "👥 Настройки подгруппы и группы\n\nТекущая подгруппа: {current}\nГруппа: {group_num}\n\nВыберите нужный вариант:"
            );
            let keyboard = subgroup_keyboard_vk(view);
            present_vk(app, vk, account.id, cmid, &text, keyboard, view).await
        }
        Screen::Schedule => {
            let (text, file_count) = app
                .render_schedule_content(account, view, &loaded, refresh, "")
                .await?;
            let keyboard = schedule_keyboard_vk(
                &loaded.days,
                view,
                now.date_naive(),
                app.config.timezone,
                file_count,
                teacher,
            );
            present_vk(app, vk, account.id, cmid, &text, keyboard, view).await
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
    let peer_id = match PlatformUser::from_storage_id(storage_id) {
        PlatformUser::Vk(id) => id,
        PlatformUser::Telegram(_) => return Err(anyhow!("present_vk called for Telegram user")),
    };

    let result_cmid = if let Some(id) = cmid {
        match vk
            .edit_message(peer_id, id as i64, text, Some(&keyboard))
            .await
        {
            Ok(_) => id,
            Err(e) => {
                tracing::warn!(error=%e, "failed to edit vk message, sending new");
                vk.send_message(peer_id, text, Some(&keyboard)).await? as i32
            }
        }
    } else {
        vk.send_message(peer_id, text, Some(&keyboard)).await? as i32
    };

    if result_cmid > 0 {
        app.storage.save_view(storage_id, result_cmid, view).await?;
    }
    Ok(())
}

pub async fn run(app: Arc<App>, vk: Arc<VkClient>, cancel: CancellationToken) {
    tracing::info!(group_id = vk.group_id(), "vk bot loop started");
    let mut server_info: Option<LongPollServer> = None;

    while !cancel.is_cancelled() {
        if server_info.is_none() {
            match vk.get_long_poll_server().await {
                Ok(info) => {
                    tracing::info!(ts = %info.ts, "vk long poll server obtained");
                    server_info = Some(info);
                }
                Err(e) => {
                    tracing::error!(error = %e, "failed to get vk long poll server; retrying in 5s");
                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        _ = tokio::time::sleep(Duration::from_secs(5)) => (),
                    }
                    continue;
                }
            }
        }

        let current = server_info.as_ref().unwrap();
        let poll_res = tokio::select! {
            _ = cancel.cancelled() => break,
            res = vk.poll(&current.server, &current.key, &current.ts, 25) => res,
        };

        match poll_res {
            Ok(resp) => {
                if let Some(failed) = resp.failed {
                    match failed {
                        1 => {
                            if let Some(ts) = resp.ts
                                && let Some(ref mut info) = server_info
                            {
                                info.ts = ts;
                            }
                        }
                        2 | 3 => {
                            tracing::warn!(
                                code = failed,
                                "vk long poll key/ts expired, re-fetching server"
                            );
                            server_info = None;
                        }
                        _ => {
                            server_info = None;
                        }
                    }
                    continue;
                }

                if let Some(ts) = resp.ts
                    && let Some(ref mut info) = server_info
                {
                    info.ts = ts;
                }

                for update in resp.updates {
                    match update.event_type.as_str() {
                        "message_new" => {
                            if let Ok(msg_obj) =
                                serde_json::from_value::<MessageNewObject>(update.object)
                            {
                                let (app, vk) = (app.clone(), vk.clone());
                                tokio::spawn(async move {
                                    if let Err(e) = handle_message(&app, &vk, msg_obj.message).await
                                    {
                                        tracing::warn!(error = %e, "error handling vk message");
                                    }
                                });
                            }
                        }
                        "message_event" => {
                            if let Ok(event_obj) =
                                serde_json::from_value::<MessageEventObject>(update.object)
                            {
                                let (app, vk) = (app.clone(), vk.clone());
                                tokio::spawn(async move {
                                    if let Err(e) = handle_callback(&app, &vk, event_obj).await {
                                        tracing::warn!(error = %e, "error handling vk callback");
                                    }
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "vk poll error; retrying in 2s");
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_secs(2)) => (),
                }
            }
        }
    }

    tracing::info!("vk bot loop stopped");
}
