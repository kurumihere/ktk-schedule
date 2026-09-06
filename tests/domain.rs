use chrono::{NaiveDate, TimeZone};
use chrono_tz::Asia::Yekaterinburg as TZ;
use ktk_schedule::{
    app::notification_due,
    config::Config,
    model::*,
    render::{self, Assets},
    storage::Account,
    telegram,
};
use serde_json::json;
use std::collections::HashMap;

fn account() -> Account {
    Account {
        id: 42,
        login: "test".into(),
        password: "secret".into(),
        group_id: 269,
        personal_subgroup: "left".into(),
        subgroup: "left".into(),
        show_all: false,
        teacher_hash: String::new(),
        notify: false,
    }
}
fn date() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 9, 1).unwrap()
}

#[test]
fn dates_and_academic_weeks_match_the_existing_interface() {
    for input in ["01.09", "1.9", "01.09.2026", "2026-09-01", ""] {
        assert_eq!(parse_date(input, date()).unwrap(), date());
    }
    assert!(parse_date("31.02", date()).is_err());
    assert_eq!(
        render::week_label(date()),
        "Неделя 1 (с 31 августа по 05 сентября)"
    );
    assert_eq!(
        week_millis(date(), TZ).unwrap(),
        TZ.with_ymd_and_hms(2026, 8, 31, 6, 0, 0)
            .unwrap()
            .timestamp_millis()
    );
    assert_eq!(day_date("2026-08-31T22:00:00Z", TZ), Some(date()));
}

#[test]
fn attachments_accept_alternate_api_shapes_without_duplicate_ids() {
    let value = json!({"Files": [{"ID": 101}, {"FileID":"102"}, {"Document":{"ID":103}}, 101, -1, 2.5], "FileID":104, "Documents":[{"DocumentID":105}], "Attachments":[{"File":{"ID":106}}]});
    assert_eq!(attachment_ids(&value), [101, 102, 103, 104, 105, 106]);
    let homework: Homework = serde_json::from_value(value).unwrap();
    assert_eq!(homework.files, [101, 102, 103, 104, 105, 106]);
}

#[test]
fn both_schedule_formats_and_null_optional_data_are_supported() {
    let old = parse_schedule(&json!([{"Date":"2026-09-01", "Subjects":[{"Pair":1,"Discipline":"Math", "ExtraData":null}]}])).unwrap();
    assert_eq!(old[0].subjects[0].extra_data, ExtraData::default());
    let new = parse_schedule(&json!([{"MaxPair":4,"DayList":[{"Date":"2026-09-01","Pairs":[{"Number":2,"Subgroups":{"right":[{"Discipline":"Math","CallPreset":8}]}}]}]}])).unwrap();
    assert_eq!(new[0].call_preset, 8);
    assert_eq!(new[0].subjects[0].pair, 2);
    assert_eq!(new[0].subjects[0].subgroup, "right");
    assert!(parse_schedule(&json!([])).unwrap().is_empty());
    assert!(parse_schedule(&json!({"error":"not a schedule"})).is_err());
    assert!(parse_schedule(&json!([{"unrelated":[]}])).is_err());
}

#[test]
fn filtering_keeps_common_lessons_and_avoids_copying_subjects() {
    let day: ScheduleDay = serde_json::from_value(json!({"Subjects":[{"Discipline":"Common","Subgroup":"middle"},{"Discipline":"Left","Subgroup":"1-я подгруппа"},{"Discipline":"Right","Subgroup":"2-я подгруппа"}]})).unwrap();
    let mut view = View::own(&account(), date());
    let visible = visible_subjects(&day, &view, false);
    assert_eq!(visible.len(), 2);
    assert!(std::ptr::eq(visible[0], &day.subjects[0]));
    assert_eq!(visible[1].discipline, "Left");
    view.show_all = true;
    assert_eq!(visible_subjects(&day, &view, false).len(), 3);
}

#[test]
fn personal_scope_uses_the_linked_subgroup_not_a_global_default() {
    let mut a = account();
    a.personal_subgroup = "right".into();
    a.subgroup = "right".into();
    let mut v = View::own(&a, date());
    assert_eq!(v.scope(&a), "personal");
    v.subgroup = "left".into();
    assert_eq!(v.scope(&a), "group:269");
    v.group = Some(270);
    v.select_date(telegram::shift(date(), 7).unwrap());
    assert_eq!(v.scope(&a), "group:270");
}

#[test]
fn zero_based_calls_match_website_times_and_durations() {
    let preset: Preset = serde_json::from_value(json!({
        "ID": 21, "Begin": "0000-01-01T08:00:00Z", "CallSet": [
            {"PairNumber":0,"Duration":45,"Break":10},
            {"PairNumber":1,"Duration":70,"Break":10},
            {"PairNumber":2,"Duration":70,"Break":30},
            {"PairNumber":3,"Duration":70,"Break":10},
            {"PairNumber":4,"Duration":30,"Break":10},
            {"PairNumber":5,"Duration":70,"Break":20}
        ]
    }))
    .unwrap();
    assert_eq!(
        render::timings(&preset),
        HashMap::from([
            (1, (480, 525)),
            (2, (535, 605)),
            (3, (615, 685)),
            (4, (715, 785)),
            (5, (795, 825)),
            (6, (835, 905))
        ])
    );
    let day: ScheduleDay = serde_json::from_value(json!({
        "Date":"2026-09-07", "CallPreset":21, "Subjects":[
            {"Pair":2,"Discipline":"Second"},
            {"Pair":3,"Discipline":"Third"},
            {"Pair":4,"Discipline":"Fourth"},
            {"Pair":5,"Discipline":"Fifth"}
        ]
    }))
    .unwrap();
    let refs = ReferenceData {
        presets: HashMap::from([(21, preset)]),
        ..Default::default()
    };
    let now = TZ.with_ymd_and_hms(2026, 9, 7, 9, 0, 0).unwrap();
    let text = render::schedule(
        &day,
        &View::own(&account(), now.date_naive()),
        &refs,
        &Assets::default(),
        false,
        now,
    );
    for expected in [
        "1 пара [45 мин] — пусто\n⏰ 08:00-08:45",
        "2 пара [70 мин] — Second\n⏰ 08:55-10:05\n⏳ идёт 5 мин, осталось 1 ч 5 мин",
        "3 пара [70 мин] — Third\n⏰ 10:15-11:25",
        "4 пара [70 мин] — Fourth\n⏰ 11:55-13:05",
        "5 пара [30 мин] — Fifth\n⏰ 13:15-13:45",
    ] {
        assert!(text.contains(expected), "missing: {expected}\n{text}");
    }
}

#[test]
fn student_schedule_text_is_preserved() {
    let day: ScheduleDay = serde_json::from_value(json!({"Date":"2026-09-01T00:00:00Z", "CallPreset":1,"MaxPair":6,"Subjects":[{"Pair":1,"Discipline":"Математика","Teacher":"Иванов И. И.","Group":"269","LectureHall":42,"Appraisal":5,"Mark":128,"ExtraData":{"Sheet":7,"Homework":{"Task":" Решить пример ","Webinar":" https://example.test/lesson ","Files":[10]}}}]})).unwrap();
    let refs = ReferenceData {
        halls: HashMap::from([(
            42,
            Hall {
                id: 42,
                number: "301".into(),
            },
        )]),
        presets: HashMap::from([(
            1,
            Preset {
                id: 1,
                begin: "1970-01-01T08:00:00Z".into(),
                call_set: vec![Call {
                    duration: 90,
                    r#break: 10,
                    pair_number: 1,
                }],
            },
        )]),
        ..Default::default()
    };
    let assets = Assets {
        documents: HashMap::from([
            (
                10,
                Document {
                    id: 10,
                    caption: "task.pdf".into(),
                    icon: "pdf".into(),
                },
            ),
            (
                11,
                Document {
                    id: 11,
                    caption: "answer.pdf".into(),
                    icon: "pdf".into(),
                },
            ),
        ]),
        submissions: HashMap::from([(7, 11)]),
    };
    let text = render::schedule(
        &day,
        &View::own(&account(), date()),
        &refs,
        &assets,
        false,
        TZ.with_ymd_and_hms(2026, 9, 1, 8, 30, 0).unwrap(),
    );
    assert_eq!(
        text,
        "📅 Сегодня — 01.09.2026\n\n1 пара [90 мин] — Математика\n⏰ 08:00-09:30\n⏳ идёт 30 мин, осталось 1 ч\n📊 Оценка: 5+\n👤 Иванов И. И.\n👥 Группа: 269\n🏫 Кабинет: 301\nЗадание: Решить пример\nВебинар: https://example.test/lesson\n📎 1 файл\n  • task.pdf\n📤 1 файл (моя работа)\n  • answer.pdf"
    );
}

#[test]
fn teacher_output_omits_personal_grades_and_homework_submissions() {
    let day: ScheduleDay = serde_json::from_value(json!({"Date":"2026-09-01","Subjects":[{"Pair":2,"Discipline":"Math","Teacher":"Other","Group":"269","LectureHall":3,"Appraisal":5,"Mark":16,"ExtraData":{"Sheet":7}}]})).unwrap();
    let assets = Assets {
        submissions: HashMap::from([(7, 11)]),
        ..Default::default()
    };
    let text = render::schedule(
        &day,
        &View::own(&account(), date()),
        &ReferenceData::default(),
        &assets,
        true,
        TZ.with_ymd_and_hms(2026, 9, 2, 12, 0, 0).unwrap(),
    );
    assert_eq!(
        text,
        "📅 01.09.2026\n\n1 пара — пусто\n\n2 пара — Math\n👥 Группа: 269\n🏫 Кабинет: 3"
    );
}

#[test]
fn keyboard_keeps_labels_order_and_callback_data() {
    let days: Vec<ScheduleDay> =
        serde_json::from_value(json!([{"Date":"2026-09-01"},{"Date":"2026-09-02"}])).unwrap();
    let keyboard = serde_json::to_value(telegram::schedule_keyboard(
        &days,
        &View::own(&account(), date()),
        date(),
        TZ,
        2,
        false,
    ))
    .unwrap();
    assert_eq!(
        keyboard["inline_keyboard"][0],
        json!([
            {"text":"⬅️ неделя","callback_data":"schedule:week:prev"},
            {"text":"Неделя 1 (с 31 августа по 05 сентября)","callback_data":"schedule:week:select"},
            {"text":"неделя ➡️","callback_data":"schedule:week:next"}
        ])
    );
    assert_eq!(
        keyboard["inline_keyboard"][1][1],
        json!({"text":"🔄 Обновить","callback_data":"schedule:refresh"})
    );
    assert_eq!(keyboard["inline_keyboard"][3][0]["text"], "✅ 1 подгруппа");
    assert_eq!(
        keyboard["inline_keyboard"][4][0]["text"],
        "📎 Скачать файлы (2)"
    );
    assert_eq!(keyboard["inline_keyboard"][5][0]["text"], "✅ Вт 01.09");
    for row in keyboard["inline_keyboard"].as_array().unwrap() {
        for button in row.as_array().unwrap() {
            assert!(button["callback_data"].as_str().unwrap().len() <= 64);
        }
    }
}

#[test]
fn messages_handle_unicode_limits() {
    let long = "🦀".repeat(3000);
    let result = render::fit_text(&long, 4096);
    assert!(result.encode_utf16().count() <= 4096);
    assert!(result.ends_with('…'));
    assert_eq!(render::fit_text("Привет!", 4096), "Привет!");
}

#[test]
fn notifications_only_catch_up_for_two_minutes() {
    let time = |h, m, s| chrono::NaiveTime::from_hms_opt(h, m, s).unwrap();
    let target = time(7, 30, 0);
    assert!(!notification_due(time(7, 29, 59), target));
    assert!(notification_due(time(7, 30, 0), target));
    assert!(notification_due(time(7, 32, 0), target));
    assert!(!notification_due(time(7, 32, 1), target));
}

#[test]
fn invalid_configuration_fails_before_network_requests() {
    let mut values = HashMap::from([
        ("BOT_TOKEN", "123456:TEST"),
        ("CREDENTIALS_SECRET", "01234567890123456789012345678901"),
    ]);
    assert!(Config::from_values(|k| values.get(k).map(|v| v.to_string())).is_ok());
    for (key, value) in [
        ("NOTIFY_TIME", "25:00"),
        ("TIMEZONE", "Nowhere"),
        ("DEFAULT_SUBGROUP", "3"),
        ("DEFAULT_GROUP_ID", "-1"),
        ("KTK_BASE_URL", "file:///tmp"),
        ("BOT_TOKEN", "invalid"),
    ] {
        let previous = values.insert(key, value);
        assert!(
            Config::from_values(|k| values.get(k).map(|v| v.to_string())).is_err(),
            "{key}"
        );
        if let Some(previous) = previous {
            values.insert(key, previous);
        } else {
            values.remove(key);
        }
    }
}
