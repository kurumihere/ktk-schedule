use crate::model::*;
use chrono::{DateTime, Datelike, Days, NaiveDate, Timelike};
use chrono_tz::Tz;
use std::{
    collections::{HashMap, HashSet},
    fmt::Write,
};

// Every message the bot sends lives here: short, neutral, no greetings, no addressing.
// Telegram and VK share them; VK only differs where the platform does.

pub const TELEGRAM_BOT: &str = "https://t.me/ktk45_schedule_bot";

pub const HELP: &str = "\
/login логин пароль - вход
/schedule [дата] - расписание
/notify_on, /notify_off - утренняя рассылка
/logout - выход и удаление данных";

pub const VK_HELP: &str = "\
/login логин пароль - вход
/schedule [дата] - расписание
/notify_on, /notify_off - утренняя рассылка
/logout - выход и удаление данных

Telegram: https://t.me/ktk45_schedule_bot";

pub const LOGIN_FIRST: &str = "Нужен вход: /login логин пароль";
/// The site ended the session; with no stored password only the user can sign in again.
pub const LOGIN_AGAIN: &str = "Сессия истекла. Нужен вход: /login логин пароль";
pub const LOGIN_USAGE: &str = "Формат: /login логин пароль";
pub const LOGIN_CANCELLED: &str = "Вход отменён: сообщение с паролем не удалось удалить.";
pub const LOGIN_REJECTED: &str = "Неверный логин или пароль.";
pub const SITE_DOWN: &str = "Сайт недоступен.";
pub const TOO_MANY_LOGINS: &str = "Слишком много попыток входа.";
pub const TOO_MANY_REQUESTS: &str = "Слишком много запросов.";
pub const LOGGED_OUT: &str = "Выход выполнен, данные удалены.";
pub const PRIVATE_ONLY: &str = "Только в личном чате с ботом.";
pub const VK_PRIVATE_ONLY: &str = "Только в личных сообщениях сообщества.";
/// VK snackbar: a plain string inside the event JSON.
pub const VK_PRIVATE_BUTTONS: &str =
    r#"{"type":"show_snackbar","text":"Только в личных сообщениях"}"#;
pub const FOREIGN_BUTTONS: &str = "Кнопки только для владельца, в личном чате.";
pub const STALE_MESSAGE: &str = "Сообщение устарело: /schedule";
pub const REQUEST_FAILED: &str = "Не удалось выполнить запрос.";
pub const SCHEDULE_FAILED: &str = "Не удалось получить расписание.";
pub const TIMEOUT: &str = "Сайт не ответил вовремя.";
pub const DATE_FORMAT: &str = "Формат даты: 01.09 или 2026-09-01.";
pub const NOTIFY_ON: &str = "Утренняя рассылка включена.";
pub const NOTIFY_OFF: &str = "Утренняя рассылка выключена.";
pub const GROUP_PROMPT: &str = "Номер группы, например 269:";
pub const GROUP_FORMAT: &str = "Номер группы - число, например 269.";
pub const UNKNOWN_COMMAND: &str = "Неизвестная команда. Список: /start";
pub const NO_ACCESS: &str = "Нет доступа.";
pub const ANNOUNCE_HINT: &str = "Рассылка: /announce ответом на сообщение.";
pub const ANNOUNCE_USAGE: &str = "Формат: /announce текст, или /announce ответом на сообщение.";
pub const ANNOUNCE_OFF: &str = "Рассылка отключена: не задан OWNER_TELEGRAM_ID.";
pub const VK_ANNOUNCE_OFF: &str = "Рассылка отключена: не задан OWNER_VK_ID.";
pub const NO_RECIPIENTS: &str = "Получателей нет.";
pub const NO_FILES: &str = "Файлов нет.";
pub const FILES_OFFLINE: &str = "Файлы недоступны: сайт не отвечает.";
pub const VK_FILES: &str = "Файлы доступны в Telegram: https://t.me/ktk45_schedule_bot";
pub const FILES_HEADER: &str = "📎 Файлы:";
pub const NO_PAIRS_THIS_WEEK: &str = "Пар на неделе нет.";
pub const STALE_SCHEDULE: &str = "Сайт недоступен, сохранённая версия.";

pub fn signed_in(account: &crate::storage::Account) -> String {
    if account.teacher_hash.is_empty() {
        format!(
            "Вход выполнен: группа {}, {}.",
            account.group_id,
            subgroup_label(&account.subgroup)
        )
    } else {
        "Вход выполнен: преподаватель.".into()
    }
}

/// VK cannot delete the user's message: the password stays in the chat history.
pub fn vk_signed_in(account: &crate::storage::Account) -> String {
    format!(
        "{} Сообщение с паролем стоит удалить из чата.",
        signed_in(account)
    )
}

pub fn stats(users: i64, notify: i64, sessions: u64, uptime_seconds: u64, tz: Tz) -> String {
    format!(
        "📊 Статистика\nПользователи: {users}\nРассылка: {notify}\nСессии в памяти: {sessions}\nАптайм: {}\nЧасовой пояс: {tz}",
        uptime(uptime_seconds)
    )
}

pub fn announce_done(sent: usize, failed: usize) -> String {
    format!("Доставлено: {sent}, ошибок: {failed}.")
}

pub fn file_failed(name: &str) -> String {
    format!("Не удалось получить файл \"{name}\".")
}

pub fn no_pairs(date: NaiveDate) -> String {
    let note = if date.weekday() == chrono::Weekday::Sun {
        "Воскресенье, выходной."
    } else {
        "Пар нет."
    };
    format!("📅 {}\n\n{note}", date.format("%d.%m.%Y"))
}

pub fn group_header(group: i64) -> String {
    format!("📋 Группа {group}\n\n")
}

/// VK settings menu: what is selected now, then the buttons.
pub fn vk_menu(view: &View, group: i64, teacher: bool) -> String {
    if teacher {
        return week_label(view.week());
    }
    let subgroup = if view.show_all {
        "обе подгруппы"
    } else if view.subgroup == "left" {
        "1 подгруппа"
    } else {
        "2 подгруппа"
    };
    format!("Группа {group}, {subgroup}\n{}", week_label(view.week()))
}

const MONTHS: [&str; 12] = [
    "января",
    "февраля",
    "марта",
    "апреля",
    "мая",
    "июня",
    "июля",
    "августа",
    "сентября",
    "октября",
    "ноября",
    "декабря",
];
const DAYS: [&str; 7] = ["Пн", "Вт", "Ср", "Чт", "Пт", "Сб", "Вс"];

/// Academic week number with the Monday and Saturday it spans.
fn academic_week(date: NaiveDate) -> (i64, NaiveDate, NaiveDate) {
    let start = week_start(date);
    let number = (start - academic_year_start(start)).num_days() / 7 + 1;
    let end = start.checked_add_days(Days::new(5)).unwrap_or(start);
    (number, start, end)
}

pub fn week_label(date: NaiveDate) -> String {
    let (number, start, end) = academic_week(date);
    format!(
        "Неделя {number} (с {:02} {} по {:02} {})",
        start.day(),
        MONTHS[start.month0() as usize],
        end.day(),
        MONTHS[end.month0() as usize]
    )
}

pub fn short_week_label(date: NaiveDate) -> String {
    let (number, start, end) = academic_week(date);
    format!(
        "Неделя {number} ({:02}.{:02} - {:02}.{:02})",
        start.day(),
        start.month(),
        end.day(),
        end.month()
    )
}

pub fn short_day(day: &ScheduleDay, tz: Tz) -> String {
    day_date(&day.date, tz)
        .map(|d| {
            format!(
                "{} {}",
                DAYS[d.weekday().num_days_from_monday() as usize],
                d.format("%d.%m")
            )
        })
        .unwrap_or_else(|| day.date.clone())
}

#[derive(Default)]
pub struct Assets {
    pub documents: HashMap<i64, Document>,
    pub submissions: HashMap<i64, i64>,
}

pub fn duration(seconds: i64) -> String {
    let minutes = ((seconds + 30) / 60).max(0);
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m} мин"),
        (h, 0) => format!("{h} ч"),
        (h, m) => format!("{h} ч {m} мин"),
    }
}

pub fn uptime(seconds: u64) -> String {
    let mins = seconds / 60;
    if mins >= 1440 {
        format!("{}д {}ч {}мин", mins / 1440, mins % 1440 / 60, mins % 60)
    } else if mins >= 60 {
        format!("{}ч {}мин", mins / 60, mins % 60)
    } else {
        format!("{mins}мин")
    }
}

pub fn mark(value: i64, absence: &HashMap<i64, String>) -> String {
    match value {
        128 => "+".into(),
        256 => "-".into(),
        16 => "Н".into(),
        32 => "О".into(),
        8 => "🪲".into(),
        n => absence
            .get(&n)
            .map(|s| format!("{} {s}", if matches!(n, 4 | 11) { "Б" } else { "Н" }))
            .unwrap_or_else(|| n.to_string()),
    }
}

fn pair_type(t: &PairType) -> String {
    let emoji = match t.billing_type.as_str() {
        "Theoretical" => "📚 ",
        "Practice" => "🔬 ",
        "IndependentWork" => "📘 ",
        "Certification" => "📝 ",
        "Consultation" => "💬 ",
        "CourseWork" => "📄 ",
        _ => "",
    };
    format!("{emoji}{}", t.name)
}

pub fn file_icon(icon: &str) -> &'static str {
    for (part, emoji) in [
        ("pdf", "📄"),
        ("image", "🖼"),
        ("word", "📝"),
        ("excel", "📊"),
        ("powerpoint", "📽"),
        ("archive", "📦"),
    ] {
        if icon.contains(part) {
            return emoji;
        }
    }
    "📎"
}

pub fn timings(preset: &Preset) -> HashMap<i64, (i64, i64)> {
    let time = preset
        .begin
        .split_once('T')
        .map(|(_, t)| t)
        .unwrap_or(&preset.begin);
    let mut parts = time.split(':');
    let mut minutes = parts
        .next()
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0)
        * 60
        + parts
            .next()
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0);
    let mut timings = HashMap::new();
    for (i, call) in preset.call_set.iter().enumerate() {
        // The website uses array positions; API PairNumber starts at zero.
        let number = i as i64 + 1;
        timings.insert(number, (minutes, minutes + call.duration));
        minutes += call.duration + call.r#break;
    }
    timings
}

pub fn schedule(
    day: &ScheduleDay,
    view: &View,
    refs: &ReferenceData,
    assets: &Assets,
    teacher: bool,
    is_personal: bool,
    now: DateTime<Tz>,
) -> String {
    let date = day_date(&day.date, now.timezone());
    let today = date == Some(now.date_naive());
    let label = date
        .map(|d| d.format("%d.%m.%Y").to_string())
        .unwrap_or_else(|| day.date.clone());
    let mut out = format!("📅 {}{label}\n\n", if today { "Сегодня - " } else { "" });
    let subjects = visible_subjects_scoped(day, view, teacher, is_personal);
    let last = subjects
        .iter()
        .map(|s| s.pair)
        .max()
        .unwrap_or(0)
        .clamp(0, 100);
    if last == 0 {
        out.push_str("Пар нет.");
        return out;
    }
    let mut buildings = HashSet::new();
    for s in &subjects {
        if let Some(h) = refs.halls.get(&s.lecture_hall) {
            let (_, b) = h.parts();
            if !b.is_empty() {
                buildings.insert(b);
            }
        }
    }
    let show_building = buildings.len() > 1;
    let times = refs
        .presets
        .get(&day.call_preset)
        .map(timings)
        .unwrap_or_default();
    for pair in 1..=last {
        let items: Vec<_> = subjects.iter().filter(|s| s.pair == pair).collect();
        if items.is_empty() {
            write!(out, "{pair} пара").unwrap();
            if let Some((start, end)) = times.get(&pair) {
                write!(out, " [{} мин]", end - start).unwrap();
            }
            out.push_str(" - пусто\n");
            write_time(&mut out, times.get(&pair), false, now);
            out.push('\n');
        }
        for s in items {
            write!(out, "{pair} пара").unwrap();
            if let Some((start, end)) = times.get(&pair) {
                write!(out, " [{} мин]", end - start).unwrap();
            }
            if view.show_all
                && let Some(sub) = personal_subgroup(&s.subgroup)
            {
                write!(out, " [{}]", if sub == "left" { 1 } else { 2 }).unwrap();
            }
            writeln!(out, " - {}", s.discipline).unwrap();
            write_time(&mut out, times.get(&pair), today, now);
            if let Some(t) = refs
                .types
                .get(&s.extra_data.lecture_type)
                .or_else(|| refs.types.get(&s.extended_data.pair_type))
            {
                writeln!(out, "{}", pair_type(t)).unwrap();
            }
            if !teacher {
                if s.appraisal != 0 || matches!(s.mark, 128 | 256) {
                    out.push_str("📊 Оценка: ");
                    if s.appraisal != 0 {
                        write!(out, "{}", s.appraisal).unwrap();
                    }
                    if matches!(s.mark, 128 | 256) {
                        out.push_str(&mark(s.mark, &refs.absence));
                    }
                    out.push('\n');
                }
                if s.mark != 0 && !matches!(s.mark, 128 | 256) {
                    writeln!(out, "📊 Отметка: {}", mark(s.mark, &refs.absence)).unwrap();
                }
                if !s.teacher.is_empty() {
                    writeln!(out, "👤 {}", s.teacher).unwrap();
                }
            }
            if teacher && !s.group.is_empty() {
                writeln!(out, "👥 Группа: {}", s.group).unwrap();
            }
            if s.lecture_hall > 0 || refs.halls.contains_key(&s.lecture_hall) {
                writeln!(
                    out,
                    "🏫 Кабинет: {}",
                    refs.halls
                        .get(&s.lecture_hall)
                        .filter(|h| !h.number.is_empty())
                        .map(|h| h.display(show_building))
                        .unwrap_or_else(|| s.lecture_hall.to_string())
                )
                .unwrap();
            }
            let h = &s.extra_data.homework;
            for (label, value) in [("Задание", &h.task), ("Вебинар", &h.webinar)] {
                if let Some(value) = value.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                    writeln!(out, "{label}: {value}").unwrap();
                }
            }
            let n = h.files.len();
            if n > 0 {
                let suffix = if n % 10 == 1 && n % 100 != 11 {
                    "файл"
                } else if (2..=4).contains(&(n % 10)) && !(10..20).contains(&(n % 100)) {
                    "файла"
                } else {
                    "файлов"
                };
                writeln!(out, "📎 {n} {suffix}").unwrap();
                if n <= 20 {
                    for id in &h.files {
                        write_filename(&mut out, *id, assets);
                    }
                }
            }
            if !teacher
                && let Some(id) = assets
                    .submissions
                    .get(&s.extra_data.sheet)
                    .filter(|id| **id > 0)
            {
                out.push_str("📤 1 файл (моя работа)\n");
                write_filename(&mut out, *id, assets);
            }
            out.push('\n');
        }
    }
    out.trim_end().into()
}

fn write_filename(out: &mut String, id: i64, assets: &Assets) {
    if let Some(doc) = assets.documents.get(&id) {
        writeln!(out, "  • {}", doc.caption).unwrap();
    }
}

fn write_time(out: &mut String, time: Option<&(i64, i64)>, today: bool, now: DateTime<Tz>) {
    let Some(&(start, end)) = time else { return };
    writeln!(
        out,
        "⏰ {:02}:{:02} - {:02}:{:02}",
        start / 60,
        start % 60,
        end / 60,
        end % 60
    )
    .unwrap();
    if today {
        let seconds = i64::from(now.num_seconds_from_midnight());
        if seconds > start * 60 && seconds < end * 60 {
            writeln!(
                out,
                "⏳ идёт {}, осталось {}",
                duration(seconds - start * 60),
                duration(end * 60 - seconds)
            )
            .unwrap();
        } else if seconds < start * 60 && start * 60 - seconds <= 3600 {
            writeln!(out, "⏳ начнётся через {}", duration(start * 60 - seconds)).unwrap();
        }
    }
}

/// Telegram counts message limits in UTF-16 units; never split a Unicode scalar.
pub fn fit_text(text: &str, limit: usize) -> String {
    if text.encode_utf16().count() <= limit {
        return text.into();
    }
    let mut used = 0;
    let mut out = String::new();
    for c in text.chars() {
        if used + c.len_utf16() + 1 > limit {
            break;
        }
        out.push(c);
        used += c.len_utf16();
    }
    if limit > 0 {
        out.push('…');
    }
    out
}
