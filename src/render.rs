use crate::model::*;
use chrono::{DateTime, Datelike, Days, NaiveDate, Timelike};
use chrono_tz::Tz;
use std::{collections::HashMap, fmt::Write};

pub const HELP: &str = "Привет! Я ktk-schedule\n\nКоманды:\n/start (Показать список команд)\n/login логин пароль (Авторизоваться в workspace)\n/schedule [дата] (Показать расписание на текущую неделю или дату)\n/notify_on || _off (Включить || Отключить утренние уведомления)\n";
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

pub fn week_label(date: NaiveDate) -> String {
    let start = week_start(date);
    let mut academic = week_start(NaiveDate::from_ymd_opt(start.year(), 9, 1).unwrap());
    if start < academic {
        academic = week_start(NaiveDate::from_ymd_opt(start.year() - 1, 9, 1).unwrap());
    }
    let number = (start - academic).num_days() / 7 + 1;
    let end = start.checked_add_days(Days::new(5)).unwrap_or(start);
    format!(
        "Неделя {number} (с {:02} {} по {:02} {})",
        start.day(),
        MONTHS[start.month0() as usize],
        end.day(),
        MONTHS[end.month0() as usize]
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
    now: DateTime<Tz>,
) -> String {
    let date = day_date(&day.date, now.timezone());
    let today = date == Some(now.date_naive());
    let label = date
        .map(|d| d.format("%d.%m.%Y").to_string())
        .unwrap_or_else(|| day.date.clone());
    let mut out = format!("📅 {}{label}\n\n", if today { "Сегодня — " } else { "" });
    let subjects = visible_subjects(day, view, teacher);
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
            out.push_str(" — пусто\n");
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
            writeln!(out, " — {}", s.discipline).unwrap();
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
            if !s.group.is_empty() {
                writeln!(out, "👥 Группа: {}", s.group).unwrap();
            }
            writeln!(
                out,
                "🏫 Кабинет: {}",
                refs.halls
                    .get(&s.lecture_hall)
                    .filter(|h| !h.number.is_empty())
                    .map(|h| h.number.clone())
                    .unwrap_or_else(|| s.lecture_hall.to_string())
            )
            .unwrap();
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
        "⏰ {:02}:{:02}-{:02}:{:02}",
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
