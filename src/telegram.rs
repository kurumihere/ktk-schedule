use crate::{
    app::{App, login_required},
    model::*,
    render,
    storage::Account,
    workspace::CredentialsRejected,
};
use anyhow::{Result, anyhow};
use chrono::NaiveDate;
use chrono_tz::Tz;
use futures::{StreamExt, stream};
use std::sync::Arc;
use teloxide::{
    prelude::*,
    types::{
        BotCommand, BotCommandScope, InlineKeyboardButton as Button,
        InlineKeyboardMarkup as Keyboard, InlineQuery, InlineQueryResult, InlineQueryResultArticle,
        InlineQueryResultsButton, InlineQueryResultsButtonKind, InputFile, InputMessageContent,
        InputMessageContentText, MessageId,
    },
};

fn button(text: impl Into<String>, data: impl Into<String>) -> Button {
    Button::callback(text, data)
}

pub fn schedule_keyboard(
    days: &[ScheduleDay],
    view: &View,
    today: NaiveDate,
    tz: Tz,
    files: usize,
    teacher: bool,
    is_personal: bool,
) -> Keyboard {
    let index = days
        .iter()
        .position(|d| day_date(&d.date, tz) == Some(view.date));
    let selected_today = view.date == today && index.is_some();
    let mut rows = vec![
        vec![
            button("⬅️ неделя", "schedule:week:prev"),
            button(render::week_label(view.week()), "schedule:week:select"),
            button("неделя ➡️", "schedule:week:next"),
        ],
        vec![
            button(
                if index.is_none_or(|i| i == 0) {
                    "⛔"
                } else {
                    "⬅️"
                },
                "schedule:prev",
            ),
            button(
                if selected_today {
                    "🔄 Обновить"
                } else {
                    "Сегодня"
                },
                if selected_today {
                    "schedule:refresh"
                } else {
                    "schedule:today"
                },
            ),
            button(
                if index.is_some_and(|i| i + 1 >= days.len()) || days.is_empty() {
                    "⛔"
                } else {
                    "➡️"
                },
                "schedule:next",
            ),
        ],
    ];
    if view.group.is_some() {
        rows.push(vec![
            button(
                if teacher {
                    "👤 Своё расписание"
                } else {
                    "🏠 Своя группа"
                },
                "schedule:my",
            ),
            button("🔍 Другая группа", "schedule:group:select"),
        ]);
    } else {
        rows.push(vec![button(
            "🔍 Расписание группы",
            "schedule:group:select",
        )]);
    }
    if !teacher {
        rows.push(vec![
            button(
                if !view.show_all && view.subgroup == "left" {
                    "✅ 1 подгруппа"
                } else {
                    "1 подгруппа"
                },
                "schedule:subgroup:left",
            ),
            button(
                if !view.show_all && view.subgroup == "right" {
                    "✅ 2 подгруппа"
                } else {
                    "2 подгруппа"
                },
                "schedule:subgroup:right",
            ),
            button(
                if view.show_all {
                    "✅ Обе"
                } else {
                    "Обе"
                },
                "schedule:subgroup:all",
            ),
        ]);
    }
    if files > 0 {
        rows.push(vec![button(
            format!("📎 Скачать файлы ({files})"),
            "schedule:download",
        )]);
    }
    // Days without pairs get no button; `schedule:day:{i}` keeps the index into `days`.
    for (i, day) in days
        .iter()
        .enumerate()
        .filter(|(_, day)| !non_school_day_scoped(day, view, teacher, is_personal))
    {
        let label = format!(
            "{}{}",
            if index == Some(i) { "✅ " } else { "" },
            render::short_day(day, tz)
        );
        rows.push(vec![button(label, format!("schedule:day:{i}"))]);
    }
    Keyboard::new(rows)
}

pub fn week_keyboard(view: &View, tz: Tz) -> Result<Keyboard> {
    let center = shift(view.week(), i64::from(view.week_offset) * 7)?;
    let mut rows = vec![vec![
        button("⬅️", "schedule:week:page:-1"),
        button("Назад", "schedule:back"),
        button("➡️", "schedule:week:page:1"),
    ]];
    for delta in -2..=2 {
        let date = shift(center, delta * 7)?;
        rows.push(vec![button(
            format!(
                "{}{}",
                if date == view.week() { "✅ " } else { "" },
                render::week_label(date)
            ),
            format!("schedule:week:open:{}", week_millis(date, tz)?),
        )]);
    }
    rows.push(vec![button("Текущая неделя", "schedule:week:today")]);
    Ok(Keyboard::new(rows))
}

/// The reply for a failed request: a lost session needs the user, anything else just failed.
pub fn failure(error: &anyhow::Error) -> &'static str {
    if login_required(error) {
        render::LOGIN_AGAIN
    } else {
        render::SCHEDULE_FAILED
    }
}

/// The reply for a failed sign-in: wrong credentials, or the site did not answer.
pub fn login_failure(error: &anyhow::Error) -> &'static str {
    if error.is::<CredentialsRejected>() {
        render::LOGIN_REJECTED
    } else {
        render::SITE_DOWN
    }
}

pub fn command(text: &str) -> (&str, &str) {
    let text = text.trim();
    text.split_once(char::is_whitespace)
        .map(|(c, a)| (c, a.trim()))
        .unwrap_or((text, ""))
}

/// Which subgroup an inline query asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineSubgroup {
    /// The account's own setting (only without a group number).
    Own,
    Both,
    One(&'static str),
}

/// An inline query: `[group] [subgroup] [date or day]`, e.g. `369 1`, `369 2 05.11`,
/// `369 1 чт`, `369 обе завтра`.
/// Without a group number it is the own group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InlineRequest {
    pub group: Option<i64>,
    pub subgroup: InlineSubgroup,
    pub date: Option<NaiveDate>,
}

/// `None` for anything else: such queries get no results.
pub fn inline_request(query: &str, today: NaiveDate) -> Option<InlineRequest> {
    let mut words = query.split_whitespace().peekable();
    let group = match words.peek().map(|word| word.parse::<i64>()) {
        Some(Ok(group)) if (1..=100_000).contains(&group) => {
            words.next();
            Some(group)
        }
        Some(Ok(_)) => return None,
        _ => None,
    };
    let mut request = InlineRequest {
        group,
        subgroup: if group.is_some() {
            InlineSubgroup::Both
        } else {
            InlineSubgroup::Own
        },
        date: None,
    };
    for word in words {
        let lower = word.to_lowercase();
        if matches!(lower.as_str(), "группа" | "гр" | "подгруппа" | "подгр") {
            continue;
        }
        if matches!(lower.as_str(), "обе" | "все" | "all" | "both") {
            request.subgroup = InlineSubgroup::Both;
        } else if let Some(subgroup) = personal_subgroup(word) {
            request.subgroup = InlineSubgroup::One(subgroup);
        } else if let Some(day) = parse_day(word, today) {
            request.date = Some(day);
        } else {
            request.date = Some(parse_date(word, today).ok()?);
        }
    }
    Some(request)
}

/// Telegram drops an inline query after a few seconds; a later answer is wasted.
const INLINE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

enum Inline {
    Articles(Vec<InlineQueryResult>),
    Login,
    Nothing,
}

/// Inline mode: another group's timetable for any chat, through the asking user's own site
/// session. Only the `group:` scope is shown, so nothing personal reaches a shared chat.
pub async fn handle_inline(app: Arc<App>, query: InlineQuery) -> Result<()> {
    let id = query.from.id.0 as i64;
    let work = inline_inner(&app, id, &query.query);
    let outcome = tokio::select! {
        _ = app.cancel.cancelled() => return Ok(()),
        result = tokio::time::timeout(INLINE_DEADLINE, work) => result,
    };
    let login = InlineQueryResultsButton {
        text: render::INLINE_LOGIN.into(),
        kind: InlineQueryResultsButtonKind::StartParameter("login".into()),
    };
    let answer = app
        .bot
        .answer_inline_query(query.id, Vec::<InlineQueryResult>::new())
        // Results depend on the user's session; a short cache keeps fresh data.
        .is_personal(true)
        .cache_time(60);
    let answer = match outcome {
        Err(_) => return Ok(()),
        Ok(Ok(Inline::Articles(articles))) => answer.results(articles),
        Ok(Ok(Inline::Login)) => answer.button(login).cache_time(0),
        Ok(Err(error)) if login_required(&error) => answer.button(login).cache_time(0),
        Ok(Ok(Inline::Nothing)) => answer.cache_time(0),
        Ok(Err(error)) => {
            tracing::warn!(user_id = id, error = %error, "inline query failed");
            answer.cache_time(0)
        }
    };
    // An expired query is routine when the site is slow.
    if let Err(error) = answer.await {
        tracing::debug!(user_id = id, %error, "inline answer not delivered");
    }
    Ok(())
}

async fn inline_inner(app: &App, id: i64, query: &str) -> Result<Inline> {
    let today = app.now().date_naive();
    let Some(request) = inline_request(query, today) else {
        return Ok(Inline::Nothing);
    };
    let Some(account) = app.storage.account(id).await? else {
        return Ok(Inline::Login);
    };
    // A teacher's account has no group of its own.
    let Some(group) = request
        .group
        .or((account.teacher_hash.is_empty()).then_some(account.group_id))
    else {
        return Ok(Inline::Nothing);
    };
    if !app.allow(id, false).await {
        return Ok(Inline::Nothing);
    }
    let mut view = View::own(&account, request.date.unwrap_or(today));
    view.group = Some(group);
    match request.subgroup {
        InlineSubgroup::Own => (),
        InlineSubgroup::Both => view.show_all = true,
        InlineSubgroup::One(subgroup) => {
            view.show_all = false;
            view.subgroup = subgroup.into();
        }
    }
    let (view, loaded) = app.resolve(&account, &view, false).await?;
    // A day picker: the week's other days with pairs on top, the asked day (today by
    // default) last, right above the input field.
    let tz = app.config.timezone;
    let mut dates: Vec<NaiveDate> = loaded
        .days
        .iter()
        .filter(|day| !non_school_day_scoped(day, &view, false, false))
        .filter_map(|day| day_date(&day.date, tz))
        .filter(|date| *date != view.date)
        .collect();
    dates.sort_unstable();
    dates.dedup();
    dates.push(view.date);
    let mut articles = Vec::with_capacity(dates.len());
    for date in dates {
        let mut day = view.clone();
        day.select_date(date);
        let text = app.render_shared(&account, &day, &loaded).await?;
        let article = InlineQueryResultArticle::new(
            format!(
                "{group}:{}:{date}",
                if day.show_all { "all" } else { &day.subgroup },
            ),
            render::inline_day(date, date == today),
            InputMessageContent::Text(InputMessageContentText::new(render::fit_text(&text, 4096))),
        )
        .description(render::inline_title(group, &day));
        articles.push(InlineQueryResult::Article(article));
    }
    Ok(Inline::Articles(articles))
}

/// The "/" menu: every command in private chats (the only place they work, golden rule 1),
/// plus the owner tools in the owner's chat.
pub async fn set_commands(app: &App) -> Result<()> {
    let commands = |list: &[(&str, &str)]| -> Vec<BotCommand> {
        list.iter()
            .map(|(name, description)| BotCommand::new(*name, *description))
            .collect()
    };
    app.bot
        .set_my_commands(commands(&render::COMMANDS))
        .scope(BotCommandScope::AllPrivateChats)
        .await?;
    if app.config.owner_id > 0 {
        let owner: Vec<_> = render::COMMANDS
            .iter()
            .chain(&render::OWNER_COMMANDS)
            .copied()
            .collect();
        app.bot
            .set_my_commands(commands(&owner))
            .scope(BotCommandScope::Chat {
                chat_id: ChatId(app.config.owner_id).into(),
            })
            .await?;
    }
    Ok(())
}

fn private(message: &Message) -> bool {
    message.chat.is_private()
        && message
            .from
            .as_ref()
            .is_some_and(|u| u.id.0 as i64 == message.chat.id.0)
}
/// The bot owner, in a private chat: statistics and broadcasts never surface in groups.
fn owner(app: &App, message: &Message) -> bool {
    app.config.owner_id > 0
        && private(message)
        && message
            .from
            .as_ref()
            .is_some_and(|u| u.id.0 as i64 == app.config.owner_id)
}

async fn account(app: &App, id: i64) -> Result<Option<Account>> {
    let a = app.storage.account(id).await?;
    if a.is_none() {
        app.send(id, render::LOGIN_FIRST).await?;
    }
    Ok(a)
}

pub async fn handle_message(app: Arc<App>, message: Message) -> Result<()> {
    let id = message.chat.id.0;
    // A broadcast paces itself through Telegram's limits; only requests get the usual deadline.
    let broadcast = message
        .text()
        .is_some_and(|text| command(text).0.split('@').next() == Some("/announce"));
    let deadline = std::time::Duration::from_secs(if broadcast { 3600 } else { 120 });
    let work = message_inner(&app, &message);
    tokio::select! {
        _ = app.cancel.cancelled() => (),
        result = tokio::time::timeout(deadline, work) => {
            match result {
                Ok(Ok(())) => (),
                Ok(Err(error)) if login_required(&error) => { let _ = app.send(id, render::LOGIN_AGAIN).await; }
                Ok(Err(error)) => { tracing::error!(chat_id=id, error=%error, "message handler failed"); let _ = app.send(id, render::REQUEST_FAILED).await; }
                Err(_) => { let _ = app.send(id, render::TIMEOUT).await; }
            }
        }
    }
    Ok(())
}

async fn message_inner(app: &App, message: &Message) -> Result<()> {
    let id = message.chat.id.0;
    let _guard = app.lock_user(id).await;
    let Some(text) = message.text() else {
        return Ok(());
    };
    let (raw_command, args) = command(text);
    let cmd = if let Some((name, mention)) = raw_command.split_once('@') {
        if !mention.eq_ignore_ascii_case(&app.bot_username) {
            return Ok(());
        }
        name
    } else {
        raw_command
    };
    match cmd {
        "/start" => {
            app.send(id, render::HELP).await?;
        }
        "/my_id" => {
            app.send(
                id,
                &format!(
                    "Telegram ID: {}",
                    message.from.as_ref().map(|u| u.id.0 as i64).unwrap_or(id)
                ),
            )
            .await?;
        }
        "/login" => {
            if !private(message) {
                app.send(id, render::PRIVATE_ONLY).await?;
                return Ok(());
            }
            if app
                .bot
                .delete_message(message.chat.id, message.id)
                .await
                .is_err()
            {
                app.send(id, render::LOGIN_CANCELLED).await?;
                return Ok(());
            }
            if !app.allow(id, true).await {
                app.send(id, render::TOO_MANY_LOGINS).await?;
                return Ok(());
            }
            let args: Vec<_> = args.split_whitespace().collect();
            if args.len() != 2 {
                app.send(id, render::LOGIN_USAGE).await?;
                return Ok(());
            }
            match app.sign_in(id, args[0], args[1]).await {
                Ok(a) => {
                    app.send(id, &render::signed_in(&a)).await?;
                    // The timetable right away: no extra command needed.
                    if let Err(error) = app
                        .show(&a, &View::own(&a, app.now().date_naive()), None, false)
                        .await
                    {
                        tracing::warn!(chat_id=id,error=%error,"schedule after login failed");
                        app.send(id, failure(&error)).await?;
                    }
                }
                Err(error) => {
                    tracing::warn!(chat_id=id,error=%error,"login failed");
                    app.send(id, login_failure(&error)).await?;
                }
            }
        }
        "/logout" => {
            if !private(message) {
                app.send(id, render::PRIVATE_ONLY).await?;
                return Ok(());
            }
            app.sign_out(id).await?;
            app.send(id, render::LOGGED_OUT).await?;
        }
        "/schedule" => {
            if !private(message) {
                app.send(id, render::PRIVATE_ONLY).await?;
                return Ok(());
            }
            if !app.allow(id, false).await {
                return Ok(());
            }
            let Some(a) = account(app, id).await? else {
                return Ok(());
            };
            let date = match parse_date(args, app.now().date_naive()) {
                Ok(d) => d,
                Err(_) => {
                    app.send(id, render::DATE_FORMAT).await?;
                    return Ok(());
                }
            };
            app.pending_groups.invalidate(&id).await;
            match app.show(&a, &View::own(&a, date), None, false).await {
                Ok(()) => (),
                Err(error) => {
                    tracing::warn!(chat_id=id,error=%error,"schedule failed");
                    app.send(id, failure(&error)).await?;
                }
            }
        }
        "/notify_on" | "/notify_off" => {
            if !private(message) {
                app.send(id, render::PRIVATE_ONLY).await?;
                return Ok(());
            }
            if account(app, id).await?.is_none() {
                return Ok(());
            }
            let enabled = cmd == "/notify_on";
            app.storage.set_notify(id, enabled).await?;
            app.send(
                id,
                if enabled {
                    render::NOTIFY_ON
                } else {
                    render::NOTIFY_OFF
                },
            )
            .await?;
        }
        "/stats" => {
            if owner(app, message) {
                app.send(id, &app.stats().await?).await?;
            }
        }
        "/announce" => announce(app, message, args).await?,
        _ => {
            if private(message)
                && !text.starts_with('/')
                && let Some(message_id) = app.pending_groups.get(&id).await
            {
                let group = text
                    .trim()
                    .parse::<i64>()
                    .ok()
                    .filter(|g| (1..=100_000).contains(g));
                let Some(group) = group else {
                    app.send(id, render::GROUP_FORMAT).await?;
                    return Ok(());
                };
                if !app.allow(id, false).await {
                    return Ok(());
                }
                let Some(a) = account(app, id).await? else {
                    return Ok(());
                };
                let mut view = app
                    .storage
                    .view(id, message_id)
                    .await?
                    .unwrap_or_else(|| View::own(&a, app.now().date_naive()));
                view.group = Some(group);
                view.screen = Screen::Schedule;
                app.show(&a, &view, None, false).await?;
                app.pending_groups.invalidate(&id).await;
                return Ok(());
            }
            // In groups the bot only answers its own commands; anything else is chatter.
            if !private(message) {
                return Ok(());
            }
            app.send(
                id,
                if owner(app, message) {
                    render::ANNOUNCE_HINT
                } else {
                    render::UNKNOWN_COMMAND
                },
            )
            .await?;
        }
    }
    Ok(())
}

async fn announce(app: &App, message: &Message, text: &str) -> Result<()> {
    let id = message.chat.id.0;
    if app.config.owner_id == 0 {
        app.send(id, render::ANNOUNCE_OFF).await?;
        return Ok(());
    }
    if !owner(app, message) {
        app.send(id, render::NO_ACCESS).await?;
        return Ok(());
    }
    if text.is_empty() && message.reply_to_message().is_none() {
        app.send(id, render::ANNOUNCE_USAGE).await?;
        return Ok(());
    }
    let recipients = app.storage.ids().await?;
    let recipients: Vec<i64> = recipients
        .into_iter()
        .filter(|&id| id < crate::model::VK_ID_OFFSET)
        .collect();
    if recipients.is_empty() {
        app.send(id, render::NO_RECIPIENTS).await?;
        return Ok(());
    }
    let results = stream::iter(recipients.into_iter().map(|recipient| async move {
        if text.is_empty() {
            app.bot
                .copy_message(
                    ChatId(recipient),
                    message.chat.id,
                    message.reply_to_message().unwrap().id,
                )
                .await
                .map(|_| ())
        } else {
            app.bot
                .send_message(ChatId(recipient), render::fit_text(text, 4096))
                .await
                .map(|_| ())
        }
    }))
    .boxed()
    .buffer_unordered(4)
    .collect::<Vec<_>>()
    .await;
    let sent = results.iter().filter(|r| r.is_ok()).count();
    app.send(id, &render::announce_done(sent, results.len() - sent))
        .await?;
    Ok(())
}

pub async fn handle_callback(app: Arc<App>, query: CallbackQuery) -> Result<()> {
    let Some(message) = query.message.as_ref() else {
        let _ = app
            .bot
            .answer_callback_query(query.id)
            .text(render::STALE_MESSAGE)
            .await;
        return Ok(());
    };
    let id = message.chat().id.0;
    if !message.chat().is_private() || query.from.id.0 as i64 != id {
        app.bot
            .answer_callback_query(query.id)
            .text(render::FOREIGN_BUTTONS)
            .show_alert(true)
            .await?;
        return Ok(());
    }
    let _ = app.bot.answer_callback_query(query.id.clone()).await;
    if !app.allow(id, false).await {
        return Ok(());
    }
    let Some(data) = query.data.as_deref().filter(|s| s.starts_with("schedule:")) else {
        return Ok(());
    };
    let work = callback_inner(&app, id, message.id(), data);
    tokio::select! {
        _ = app.cancel.cancelled() => (),
        result = tokio::time::timeout(std::time::Duration::from_secs(120), work) => {
            match result {
                Ok(Ok(())) => (),
                Ok(Err(error)) => { tracing::warn!(chat_id=id,error=%error,"callback failed"); let _ = app.send(id, failure(&error)).await; }
                Err(_) => { let _ = app.send(id, render::TIMEOUT).await; }
            }
        }
    }
    Ok(())
}

async fn callback_inner(app: &App, id: i64, message: MessageId, data: &str) -> Result<()> {
    let _guard = app.lock_user(id).await;
    let Some(a) = account(app, id).await? else {
        return Ok(());
    };
    let Some(mut view) = app.storage.view(id, message.0).await? else {
        app.send(id, render::STALE_MESSAGE).await?;
        return Ok(());
    };
    let today = app.now().date_naive();
    let mut refresh = false;
    match data {
        "schedule:group:select" => {
            view.screen = Screen::GroupInput;
            app.present(
                id,
                Some(message),
                render::GROUP_PROMPT,
                Keyboard::new(vec![vec![button("↩️ Назад", "schedule:back")]]),
                &view,
            )
            .await?;
            app.pending_groups.insert(id, message.0).await;
            return Ok(());
        }
        "schedule:week:select" => {
            view.week_offset = 0;
            view.screen = Screen::Weeks;
        }
        "schedule:week:page:-1" | "schedule:week:page:1" => {
            let delta = if data.ends_with(":-1") { -5 } else { 5 };
            view.week_offset = (view.week_offset + delta).clamp(-520, 520);
            view.screen = Screen::Weeks;
        }
        "schedule:back" => {
            view.screen = Screen::Schedule;
            app.pending_groups.invalidate(&id).await;
        }
        "schedule:my" => {
            let date = view.date;
            view = View::own(&a, date);
            if a.teacher_hash.is_empty() {
                view.subgroup = a.personal_subgroup.clone();
                view.show_all = false;
                app.storage.set_subgroup(id, &view.subgroup, false).await?;
            }
        }
        "schedule:subgroup:left" | "schedule:subgroup:right" | "schedule:subgroup:all" => {
            if !a.teacher_hash.is_empty() {
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
            if view.group.is_none() {
                app.storage
                    .set_subgroup(id, &view.subgroup, view.show_all)
                    .await?;
            }
            view.screen = Screen::Schedule;
        }
        "schedule:today" | "schedule:week:today" => {
            view.select_date(today);
            view.screen = Screen::Schedule;
        }
        "schedule:refresh" => {
            refresh = true;
            view.screen = Screen::Schedule;
        }
        "schedule:week:prev" => {
            view.select_date(shift(view.week(), -7)?);
            view.screen = Screen::Schedule;
        }
        "schedule:week:next" => {
            view.select_date(shift(view.week(), 7)?);
            view.screen = Screen::Schedule;
        }
        "schedule:download" => {
            return download(app, &a, &view).await;
        }
        _ if data.starts_with("schedule:week:open:") => {
            let millis: i64 = data.trim_start_matches("schedule:week:open:").parse()?;
            let date = chrono::DateTime::from_timestamp_millis(millis)
                .ok_or_else(|| anyhow!("invalid week"))?
                .with_timezone(&app.config.timezone)
                .date_naive();
            if !within_reach(date, today) {
                return Ok(());
            }
            view.select_date(week_start(date));
            view.week_offset = 0;
            view.screen = Screen::Schedule;
        }
        "schedule:prev" | "schedule:next" => {
            let loaded = app.load(&a, &view, false).await?;
            let current = loaded
                .days
                .iter()
                .position(|d| day_date(&d.date, app.config.timezone) == Some(view.date));
            let next = if data == "schedule:prev" {
                current.and_then(|i| i.checked_sub(1)).or_else(|| {
                    if let Some(first) = loaded
                        .days
                        .first()
                        .and_then(|d| day_date(&d.date, app.config.timezone))
                        && view.date > first
                    {
                        Some(loaded.days.len().saturating_sub(1))
                    } else {
                        None
                    }
                })
            } else {
                current.map(|i| i + 1).or(Some(0))
            };
            let Some(day) = next.and_then(|i| loaded.days.get(i)) else {
                return Ok(());
            };
            view.select_date(
                day_date(&day.date, app.config.timezone)
                    .ok_or_else(|| anyhow!("invalid schedule date"))?,
            );
            view.screen = Screen::Schedule;
            return app
                .show_loaded(&a, &view, Some(message), &loaded, false, "")
                .await;
        }
        _ if data.starts_with("schedule:day:") => {
            let index: usize = data.trim_start_matches("schedule:day:").parse()?;
            let loaded = app.load(&a, &view, false).await?;
            let Some(day) = loaded.days.get(index) else {
                return Ok(());
            };
            view.select_date(
                day_date(&day.date, app.config.timezone)
                    .ok_or_else(|| anyhow!("invalid schedule date"))?,
            );
            view.screen = Screen::Schedule;
            return app
                .show_loaded(&a, &view, Some(message), &loaded, false, "")
                .await;
        }
        _ => return Ok(()),
    }
    if view.screen == Screen::Weeks {
        app.present(
            id,
            Some(message),
            &render::week_label(view.week()),
            week_keyboard(&view, app.config.timezone)?,
            &view,
        )
        .await
    } else {
        app.show(&a, &view, Some(message), refresh).await
    }
}

async fn download(app: &App, a: &Account, view: &View) -> Result<()> {
    let loaded = app.load(a, view, false).await?;
    let Some(session) = loaded.session else {
        app.send(a.id, render::FILES_OFFLINE).await?;
        return Ok(());
    };
    let Some(day) = loaded
        .days
        .iter()
        .find(|d| day_date(&d.date, app.config.timezone) == Some(view.date))
    else {
        return Ok(());
    };
    let is_personal = view.scope(a) == "personal";
    let subjects = visible_subjects_scoped(day, view, !a.teacher_hash.is_empty(), is_personal);
    let assets = session
        .assets(&subjects, !a.teacher_hash.is_empty(), false)
        .await?;
    let ids = limited_file_ids(
        subjects
            .iter()
            .flat_map(|s| s.extra_data.homework.files.iter().copied())
            .chain(assets.submissions.values().copied()),
    )?;
    if ids.is_empty() {
        app.send(a.id, render::NO_FILES).await?;
        return Ok(());
    }
    let caption = |id: i64| {
        assets
            .documents
            .get(&id)
            .map(|d| d.caption.clone())
            .unwrap_or_else(|| format!("file_{id}"))
    };
    let mut list = render::FILES_HEADER.to_string();
    for id in &ids {
        let name = caption(*id);
        let icon = assets
            .documents
            .get(id)
            .map(|d| render::file_icon(&d.icon))
            .unwrap_or("📎");
        list.push_str(&format!("\n{icon} {name}"));
    }
    app.send(a.id, &list).await?;
    stream::iter(ids.into_iter().map(|id| {
        let (session, caption) = (&session, &caption);
        async move {
            let work = async {
                let _permit = app.downloads.acquire().await?;
                let (data, name) = session.client.download(id).await?;
                app.bot
                    .send_document(
                        ChatId(a.id),
                        InputFile::file(data.to_path_buf()).file_name(name),
                    )
                    .await?;
                Ok::<_, anyhow::Error>(())
            };
            if let Err(e) = work.await {
                tracing::warn!(document_id=id,error=%e,"file delivery failed");
                let _ = app.send(a.id, &render::file_failed(&caption(id))).await;
            }
        }
    }))
    .boxed()
    .buffer_unordered(3)
    .collect::<Vec<_>>()
    .await;
    Ok(())
}
