use crate::{app::App, model::*, render, storage::Account};
use anyhow::{Result, anyhow};
use chrono::{Days, NaiveDate};
use chrono_tz::Tz;
use futures::{StreamExt, stream};
use std::sync::Arc;
use teloxide::{
    prelude::*,
    types::{
        InlineKeyboardButton as Button, InlineKeyboardMarkup as Keyboard, InputFile, MessageId,
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
    for (i, day) in days.iter().enumerate() {
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

pub fn shift(date: NaiveDate, delta: i64) -> Result<NaiveDate> {
    let result = if delta >= 0 {
        date.checked_add_days(Days::new(delta as u64))
    } else {
        date.checked_sub_days(Days::new(delta.unsigned_abs()))
    };
    result.ok_or_else(|| anyhow!("date out of range"))
}

pub fn command(text: &str) -> (&str, &str) {
    let text = text.trim();
    text.split_once(char::is_whitespace)
        .map(|(c, a)| (c, a.trim()))
        .unwrap_or((text, ""))
}

fn private(message: &Message) -> bool {
    message.chat.is_private()
        && message
            .from
            .as_ref()
            .is_some_and(|u| u.id.0 as i64 == message.chat.id.0)
}
fn owner(app: &App, message: &Message) -> bool {
    app.config.owner_id > 0
        && message
            .from
            .as_ref()
            .is_some_and(|u| u.id.0 as i64 == app.config.owner_id)
}

async fn account(app: &App, id: i64) -> Result<Option<Account>> {
    let a = app.storage.account(id).await?;
    if a.is_none() {
        app.send(id, "Сначала авторизуйся:\n/login логин пароль")
            .await?;
    }
    Ok(a)
}

pub async fn handle_message(app: Arc<App>, message: Message) -> Result<()> {
    let id = message.chat.id.0;
    let work = message_inner(&app, &message);
    tokio::select! {
        _ = app.cancel.cancelled() => (),
        result = tokio::time::timeout(std::time::Duration::from_secs(120), work) => {
            match result {
                Ok(Ok(())) => (),
                Ok(Err(error)) => { tracing::error!(chat_id=id, error=%error, "message handler failed"); let _ = app.send(id, "Не удалось выполнить запрос. Попробуй позже.").await; }
                Err(_) => { let _ = app.send(id, "Расписание загружается слишком долго. Попробуй ещё раз.").await; }
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
                app.send(id, "Авторизация доступна только в личном чате с ботом.")
                    .await?;
                return Ok(());
            }
            let _ = app.bot.delete_message(message.chat.id, message.id).await;
            if !app.allow(id, true).await {
                app.send(id, "Слишком много попыток входа. Попробуй позже.")
                    .await?;
                return Ok(());
            }
            let args: Vec<_> = args.split_whitespace().collect();
            if args.len() != 2 {
                app.send(id, "Используй:\n/login логин пароль").await?;
                return Ok(());
            }
            match app.sign_in(id, args[0], args[1]).await {
                Ok(a) => {
                    let text = if a.teacher_hash.is_empty() {
                        format!(
                            "Авторизация успешна.\nГруппа: {}\nПодгруппа: {}\n\nТеперь напиши /schedule",
                            a.group_id,
                            subgroup_label(&a.subgroup)
                        )
                    } else {
                        "Авторизация успешна (преподаватель).\n\nТеперь напиши /schedule".into()
                    };
                    app.send(id, &text).await?;
                }
                Err(error) => {
                    tracing::warn!(chat_id=id,error=%error,"login failed");
                    app.send(id, "Не удалось войти. Проверь логин и пароль.")
                        .await?;
                }
            }
        }
        "/schedule" => {
            if !private(message) {
                app.send(
                    id,
                    "Персональное расписание доступно только в личном чате с ботом.",
                )
                .await?;
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
                    app.send(id, "Не понял дату. Используй /schedule, /schedule 01.09 или /schedule 2026-09-01").await?;
                    return Ok(());
                }
            };
            app.pending_groups.invalidate(&id).await;
            if let Err(error) = app.show(&a, &View::own(&a, date), None, false).await {
                tracing::warn!(chat_id=id,error=%error,"schedule failed");
                app.send(id, "Не удалось получить расписание. Попробуй позже.")
                    .await?;
            }
        }
        "/notify_on" | "/notify_off" => {
            if !private(message) {
                app.send(
                    id,
                    "Настройки уведомлений доступны только в личном чате с ботом.",
                )
                .await?;
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
                    "Утреннее расписание включено."
                } else {
                    "Утреннее расписание выключено."
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
                    app.send(
                        id,
                        "Не понял номер группы. Напиши просто число, например: 269",
                    )
                    .await?;
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
            app.send(
                id,
                if owner(app, message) {
                    "Чтобы разослать это сообщение, ответь на него командой /announce."
                } else {
                    "Неизвестная команда. Напиши /start"
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
        app.send(
            id,
            "Рассылка отключена. Напиши /my_id и укажи OWNER_TELEGRAM_ID в .env.",
        )
        .await?;
        return Ok(());
    }
    if !owner(app, message) {
        app.send(id, "Нет доступа.").await?;
        return Ok(());
    }
    if text.is_empty() && message.reply_to_message().is_none() {
        app.send(id, "Используй:\n/announce текст\n\nИли ответь /announce на сообщение, которое нужно разослать.").await?;
        return Ok(());
    }
    let recipients = app.storage.ids().await?;
    if recipients.is_empty() {
        app.send(
            id,
            "Некому отправлять: в базе нет авторизованных пользователей.",
        )
        .await?;
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
    app.send(
        id,
        &format!(
            "Рассылка завершена.\nДоставлено: {sent}\nОшибок: {}",
            results.len() - sent
        ),
    )
    .await?;
    Ok(())
}

pub async fn handle_callback(app: Arc<App>, query: CallbackQuery) -> Result<()> {
    let Some(message) = query.message.as_ref() else {
        let _ = app
            .bot
            .answer_callback_query(query.id)
            .text("Сообщение устарело. Напиши /schedule ещё раз.")
            .await;
        return Ok(());
    };
    let id = message.chat().id.0;
    if !message.chat().is_private() || query.from.id.0 as i64 != id {
        app.bot
            .answer_callback_query(query.id)
            .text("Персональные кнопки доступны только владельцу в личном чате.")
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
                Ok(Err(error)) => { tracing::warn!(chat_id=id,error=%error,"callback failed"); let _ = app.send(id, "Не удалось получить расписание. Попробуй позже.").await; }
                Err(_) => { let _ = app.send(id, "Расписание загружается слишком долго. Попробуй ещё раз.").await; }
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
        app.send(id, "Сообщение устарело. Напиши /schedule ещё раз.")
            .await?;
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
                "Напиши номер группы (например: 269)",
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
            if (date - today).num_days().unsigned_abs() > 3650 {
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
                current.and_then(|i| i.checked_sub(1))
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
            &format!(
                "Выбери неделю:\n\nСейчас открыта {}",
                render::week_label(view.week())
            ),
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
        app.send(a.id, "Файлы недоступны без соединения с сайтом расписания.")
            .await?;
        return Ok(());
    };
    let Some(day) = loaded
        .days
        .iter()
        .find(|d| day_date(&d.date, app.config.timezone) == Some(view.date))
    else {
        return Ok(());
    };
    let subjects = visible_subjects(day, view, !a.teacher_hash.is_empty());
    let assets = session
        .assets(&subjects, !a.teacher_hash.is_empty(), false)
        .await;
    let mut ids: Vec<_> = subjects
        .iter()
        .flat_map(|s| s.extra_data.homework.files.iter().copied())
        .chain(assets.submissions.values().copied())
        .collect();
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        app.send(a.id, "Нет файлов для скачивания.").await?;
        return Ok(());
    }
    let mut list = "📎 Файлы:".to_string();
    for id in &ids {
        let name = assets
            .documents
            .get(id)
            .map(|d| d.caption.clone())
            .unwrap_or_else(|| format!("file_{id}"));
        let icon = assets
            .documents
            .get(id)
            .map(|d| render::file_icon(&d.icon))
            .unwrap_or("📎");
        list.push_str(&format!("\n{icon} {name}"));
    }
    app.send(a.id, &list).await?;
    stream::iter(ids.into_iter().map(|id| {
        let session = &session;
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
                let _ = app
                    .send(
                        a.id,
                        &format!("Ошибка: файл \"file_{id}\" не удалось получить"),
                    )
                    .await;
            }
        }
    }))
    .boxed()
    .buffer_unordered(3)
    .collect::<Vec<_>>()
    .await;
    Ok(())
}
