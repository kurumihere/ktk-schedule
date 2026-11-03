mod support;
use chrono::NaiveDate;
use ktk_schedule::{
    app::App,
    model::{PlatformUser, Screen, View},
    render,
    vk::{self, MessageEventObject, MessageObject},
};
use serde_json::json;
use std::sync::Arc;
use support::*;
use wiremock::{
    Mock, MockServer, Request, ResponseTemplate,
    matchers::{method, path_regex},
};

pub async fn vk_mock_server() -> MockServer {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path_regex("(?i)/messages.send"))
        .respond_with(|request: &Request| {
            let _ = request;
            ResponseTemplate::new(200).set_body_json(json!({
                "response": [{
                    "peer_id": 42,
                    "message_id": 100,
                    "conversation_message_id": 100
                }]
            }))
        })
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path_regex("(?i)/messages.edit"))
        .respond_with(|request: &Request| {
            let _ = request;
            ResponseTemplate::new(200).set_body_json(json!({
                "response": 1
            }))
        })
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path_regex("(?i)/messages.sendMessageEventAnswer"))
        .respond_with(|request: &Request| {
            let _ = request;
            ResponseTemplate::new(200).set_body_json(json!({
                "response": 1
            }))
        })
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path_regex("(?i)/groups.getLongPollServer"))
        .respond_with(|request: &Request| {
            let _ = request;
            ResponseTemplate::new(200).set_body_json(json!({
                "response": {
                    "key": "test_key",
                    "server": "https://lp.test.vk/whp/123",
                    "ts": "1"
                }
            }))
        })
        .mount(&server)
        .await;

    server
}

async fn test_app(college: &College, tg_mock: &MockServer, vk_mock: &MockServer) -> Arc<App> {
    Arc::new(vk_app(college, tg_mock, vk_mock).await)
}

fn vk_message(peer_id: i64, from_id: i64, text: &str) -> MessageObject {
    MessageObject {
        id: 1,
        date: 1788202800,
        peer_id,
        from_id,
        text: text.to_string(),
        conversation_message_id: 1,
        payload: None,
        reply_message: None,
    }
}

fn vk_callback(user_id: i64, peer_id: i64, cmid: i64, data: &str) -> MessageEventObject {
    MessageEventObject {
        user_id,
        peer_id,
        event_id: "evt_123".to_string(),
        conversation_message_id: cmid,
        payload: json!({ "data": data }),
    }
}

#[tokio::test]
async fn vk_bot_private_chat_guard_rejects_conversations() {
    let college = College::start(false).await;
    let tg_mock = telegram_server().await;
    let vk_mock = vk_mock_server().await;
    let app = test_app(&college, &tg_mock, &vk_mock).await;
    let vk = app.vk.clone().unwrap();

    // peer_id != from_id (e.g. 2000000001 is a group chat)
    let msg = vk_message(2000000001, 42, "/start");
    vk::handle_message(&app, &vk, msg).await.unwrap();

    let requests = vk_mock.received_requests().await.unwrap();
    let send_req = requests
        .iter()
        .find(|r| r.url.path().contains("messages.send"))
        .unwrap();
    let body = String::from_utf8_lossy(&send_req.body);
    assert!(
        body.contains("личных+сообщениях")
            || body.contains("личных сообщениях")
            || body.contains("%D0%BB%D0%B8%D1%87%D0%BD%D1%8B%D1%85")
    );
}

#[tokio::test]
async fn vk_bot_start_and_login_flow() {
    let college = College::start(false).await;
    let tg_mock = telegram_server().await;
    let vk_mock = vk_mock_server().await;
    let app = test_app(&college, &tg_mock, &vk_mock).await;
    let vk = app.vk.clone().unwrap();

    // 1. /start
    let msg = vk_message(42, 42, "/start");
    vk::handle_message(&app, &vk, msg).await.unwrap();

    // 2. /login
    let msg = vk_message(42, 42, "/login 1000204 secret");
    vk::handle_message(&app, &vk, msg).await.unwrap();

    // Verify account saved with VK platform ID
    let storage_id = PlatformUser::Vk(42).to_storage_id();
    let account = app.storage.account(storage_id).await.unwrap().unwrap();
    assert_eq!(account.group_id, 269);

    let requests = vk_mock.received_requests().await.unwrap();
    let send_requests: Vec<_> = requests
        .iter()
        .filter(|r| r.url.path().contains("messages.send"))
        .collect();
    // /start + login confirmation + schedule
    assert!(send_requests.len() >= 3);
}

#[tokio::test]
async fn vk_bot_callback_navigation_and_day_switch() {
    let college = College::start(false).await;
    let tg_mock = telegram_server().await;
    let vk_mock = vk_mock_server().await;
    let app = test_app(&college, &tg_mock, &vk_mock).await;
    let vk = app.vk.clone().unwrap();

    let storage_id = PlatformUser::Vk(42).to_storage_id();
    let a = app.sign_in(storage_id, "user", "password").await.unwrap();
    let view = View::own(&a, NaiveDate::from_ymd_opt(2026, 9, 1).unwrap());
    app.storage.save_view(storage_id, 100, &view).await.unwrap();

    // Click next
    let cb = vk_callback(42, 42, 100, "schedule:next");
    vk::handle_callback(&app, &vk, cb).await.unwrap();

    let updated_view = app.storage.view(storage_id, 100).await.unwrap().unwrap();
    assert_eq!(
        updated_view.date,
        NaiveDate::from_ymd_opt(2026, 9, 2).unwrap()
    );

    // Verify edit_message was called
    let requests = vk_mock.received_requests().await.unwrap();
    assert!(
        requests
            .iter()
            .any(|r| r.url.path().contains("messages.edit"))
    );
    assert!(
        requests
            .iter()
            .any(|r| r.url.path().contains("messages.sendMessageEventAnswer"))
    );
}

#[tokio::test]
async fn vk_bot_subgroup_and_week_selection() {
    let college = College::start(false).await;
    let tg_mock = telegram_server().await;
    let vk_mock = vk_mock_server().await;
    let app = test_app(&college, &tg_mock, &vk_mock).await;
    let vk = app.vk.clone().unwrap();

    let storage_id = PlatformUser::Vk(42).to_storage_id();
    let a = app.sign_in(storage_id, "user", "password").await.unwrap();
    let view = View::own(&a, NaiveDate::from_ymd_opt(2026, 9, 1).unwrap());
    app.storage.save_view(storage_id, 100, &view).await.unwrap();

    // Select right subgroup
    let cb = vk_callback(42, 42, 100, "schedule:subgroup:right");
    vk::handle_callback(&app, &vk, cb).await.unwrap();

    let updated = app.storage.view(storage_id, 100).await.unwrap().unwrap();
    assert_eq!(updated.subgroup, "right");
    assert!(!updated.show_all);

    // Switch to week screen
    let cb = vk_callback(42, 42, 100, "schedule:week:select");
    vk::handle_callback(&app, &vk, cb).await.unwrap();

    let week_view = app.storage.view(storage_id, 100).await.unwrap().unwrap();
    assert_eq!(week_view.screen, Screen::Weeks);
}

#[tokio::test]
async fn vk_bot_foreign_group_input_and_my_schedule() {
    let college = College::start(false).await;
    let tg_mock = telegram_server().await;
    let vk_mock = vk_mock_server().await;
    let app = test_app(&college, &tg_mock, &vk_mock).await;
    let vk = app.vk.clone().unwrap();

    let storage_id = PlatformUser::Vk(42).to_storage_id();
    let _a = app.sign_in(storage_id, "user", "password").await.unwrap();

    // User inputs group number "270"
    let msg = vk_message(42, 42, "270");
    vk::handle_message(&app, &vk, msg).await.unwrap();

    let view = app.storage.view(storage_id, 100).await.unwrap().unwrap();
    assert_eq!(view.group, Some(270));

    // User clicks "schedule:my"
    let cb = vk_callback(42, 42, 100, "schedule:my");
    vk::handle_callback(&app, &vk, cb).await.unwrap();

    let my_view = app.storage.view(storage_id, 100).await.unwrap().unwrap();
    assert_eq!(my_view.group, None);
}

#[tokio::test]
async fn vk_bot_logout_removes_user_data() {
    let college = College::start(false).await;
    let tg_mock = telegram_server().await;
    let vk_mock = vk_mock_server().await;
    let app = test_app(&college, &tg_mock, &vk_mock).await;
    let vk = app.vk.clone().unwrap();

    let storage_id = PlatformUser::Vk(42).to_storage_id();
    let _ = app.sign_in(storage_id, "user", "password").await.unwrap();

    let msg = vk_message(42, 42, "/logout");
    vk::handle_message(&app, &vk, msg).await.unwrap();

    assert!(app.storage.account(storage_id).await.unwrap().is_none());
}

#[test]
fn vk_schedule_keyboard_strictly_obeys_vk_limits() {
    use ktk_schedule::model::ScheduleDay;
    let tz: chrono_tz::Tz = "Asia/Yekaterinburg".parse().unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 9, 1).unwrap();
    let mut view = View {
        date: today,
        week_start: today,
        group: None,
        subgroup: "left".into(),
        show_all: false,
        week_offset: 0,
        screen: Screen::Schedule,
    };

    // Test with 0 to 7 days
    for count in 0..=7 {
        let days: Vec<ScheduleDay> = (0..count)
            .map(|i| ScheduleDay {
                date: format!("2026-09-0{}", i + 1),
                call_preset: 1,
                subjects: vec![ktk_schedule::model::Subject {
                    discipline: "Математика".into(),
                    pair: 1,
                    ..Default::default()
                }],
                ..Default::default()
            })
            .collect();

        for teacher in [false, true] {
            for foreign_group in [None, Some(270)] {
                view.group = foreign_group;
                let kb = vk::schedule_keyboard_vk(&days, &view, true, tz, teacher, false);
                assert!(kb.inline);
                assert!(kb.buttons.len() <= 6, "rows exceeded: {}", kb.buttons.len());
                let total_buttons: usize = kb.buttons.iter().map(|r| r.len()).sum();
                assert!(
                    total_buttons <= 10,
                    "total buttons exceeded: {total_buttons} for count {count}, teacher {teacher}"
                );
                for row in &kb.buttons {
                    assert!(row.len() <= 5, "buttons per row exceeded: {}", row.len());
                    for btn in row {
                        if let vk::VkAction::Callback { ref label, .. } = btn.action {
                            assert!(
                                label.chars().count() <= 40,
                                "label exceeds 40 characters: '{label}'"
                            );
                        }
                    }
                }
                if foreign_group.is_none() {
                    let last_btn = kb.buttons.last().unwrap().first().unwrap();
                    if let vk::VkAction::Callback {
                        ref label,
                        ref payload,
                    } = last_btn.action
                    {
                        assert_eq!(label, "Меню");
                        assert_eq!(payload, "{\"data\":\"schedule:subgroup:menu\"}");
                    }
                }
            }
        }
    }

    // Test week keyboard limits
    let week_kb = vk::week_keyboard_vk(&view, tz).unwrap();
    assert!(week_kb.inline);
    assert!(
        week_kb.buttons.len() <= 6,
        "week rows exceeded: {}",
        week_kb.buttons.len()
    );
    let total_week: usize = week_kb.buttons.iter().map(|r| r.len()).sum();
    assert!(
        total_week <= 10,
        "week total buttons exceeded: {total_week}"
    );
    for row in &week_kb.buttons {
        for btn in row {
            if let vk::VkAction::Callback { ref label, .. } = btn.action {
                assert!(
                    label.chars().count() <= 40,
                    "week label exceeds 40 characters: '{label}'"
                );
            }
        }
    }

    // Test subgroup keyboard limits
    for teacher in [false, true] {
        let sub_kb = vk::subgroup_keyboard_vk(&view, teacher);
        assert!(sub_kb.inline);
        assert!(
            sub_kb.buttons.len() <= 6,
            "subgroup rows exceeded: {}",
            sub_kb.buttons.len()
        );
        let total_sub: usize = sub_kb.buttons.iter().map(|r| r.len()).sum();
        assert!(
            total_sub <= 10,
            "subgroup total buttons exceeded: {total_sub}"
        );
        for row in &sub_kb.buttons {
            for btn in row {
                if let vk::VkAction::Callback { ref label, .. } = btn.action {
                    assert!(
                        label.chars().count() <= 40,
                        "subgroup label exceeds 40 characters: '{label}'"
                    );
                }
            }
        }
    }
}

#[test]
fn vk_schedule_keyboard_refreshes_today_and_hides_days_without_pairs() {
    use ktk_schedule::model::{ScheduleDay, Subject};
    let tz: chrono_tz::Tz = "Asia/Yekaterinburg".parse().unwrap();
    let monday = NaiveDate::from_ymd_opt(2026, 8, 31).unwrap();
    let view = View {
        date: monday,
        week_start: monday,
        group: None,
        subgroup: "left".into(),
        show_all: false,
        week_offset: 0,
        screen: Screen::Schedule,
    };
    let pair = Subject {
        discipline: "Математика".into(),
        pair: 1,
        ..Default::default()
    };
    // Monday has a pair, Tuesday nothing, Wednesday only the other subgroup.
    let days = vec![
        ScheduleDay {
            date: "2026-08-31".into(),
            subjects: vec![pair.clone()],
            ..Default::default()
        },
        ScheduleDay {
            date: "2026-09-01".into(),
            ..Default::default()
        },
        ScheduleDay {
            date: "2026-09-02".into(),
            subjects: vec![Subject {
                subgroup: "2".into(),
                ..pair.clone()
            }],
            ..Default::default()
        },
    ];
    let data = |kb: &vk::VkKeyboard| -> Vec<Vec<String>> {
        kb.buttons
            .iter()
            .map(|row| {
                row.iter()
                    .map(|btn| match &btn.action {
                        vk::VkAction::Callback { payload, .. } => {
                            vk::extract_callback_data(&serde_json::from_str(payload).unwrap())
                                .unwrap()
                        }
                        _ => String::new(),
                    })
                    .collect()
            })
            .collect()
    };

    let kb = data(&vk::schedule_keyboard_vk(
        &days, &view, true, tz, false, false,
    ));
    assert_eq!(kb[0][1], "schedule:refresh");
    assert_eq!(kb[1], ["schedule:day:0"]);

    let kb = data(&vk::schedule_keyboard_vk(
        &days, &view, false, tz, false, false,
    ));
    assert_eq!(kb[0][1], "schedule:today");

    // The menu has no refresh button: the timetable itself has one.
    for teacher in [false, true] {
        let menu = data(&vk::subgroup_keyboard_vk(&view, teacher)).concat();
        assert!(!menu.iter().any(|d| d == "schedule:refresh"));
    }

    // Both subgroups: Wednesday keeps its index.
    let all = View {
        show_all: true,
        ..view.clone()
    };
    let kb = data(&vk::schedule_keyboard_vk(
        &days, &all, true, tz, false, false,
    ));
    assert_eq!(kb[1], ["schedule:day:0", "schedule:day:2"]);
}

#[test]
fn sunday_is_a_day_off() {
    let sunday = NaiveDate::from_ymd_opt(2026, 9, 6).unwrap();
    assert!(ktk_schedule::render::no_pairs(sunday).ends_with("Воскресенье, выходной."));
    let saturday = NaiveDate::from_ymd_opt(2026, 9, 5).unwrap();
    assert!(ktk_schedule::render::no_pairs(saturday).ends_with("Пар нет."));
}

#[tokio::test]
async fn vk_bot_admin_commands_and_announce() {
    let college = College::start(false).await;
    let tg_mock = telegram_server().await;
    let vk_mock = vk_mock_server().await;
    let app = test_app(&college, &tg_mock, &vk_mock).await;
    let vk = app.vk.clone().unwrap();

    // 1. /my_id test
    vk::handle_message(&app, &vk, vk_message(42, 42, "/my_id"))
        .await
        .unwrap();
    let requests = vk_mock.received_requests().await.unwrap();
    let last = requests
        .iter()
        .rev()
        .find(|r| r.url.path().contains("messages.send"))
        .unwrap();
    let body = String::from_utf8_lossy(&last.body);
    assert!(body.contains("42"));

    // 2. /stats by non-owner (42) -> denied
    vk::handle_message(&app, &vk, vk_message(42, 42, "/stats"))
        .await
        .unwrap();
    assert_eq!(last_text(&vk_mock).await, render::NO_ACCESS);

    // 3. /stats by owner (777) -> success
    vk::handle_message(&app, &vk, vk_message(777, 777, "/stats"))
        .await
        .unwrap();
    assert!(last_text(&vk_mock).await.starts_with("📊 Статистика"));

    // 4. /announce by non-owner -> denied
    vk::handle_message(&app, &vk, vk_message(42, 42, "/announce Привет всем"))
        .await
        .unwrap();
    assert_eq!(last_text(&vk_mock).await, render::NO_ACCESS);

    // 5. /announce by owner with empty DB -> nobody to send to
    vk::handle_message(&app, &vk, vk_message(777, 777, "/announce Привет всем"))
        .await
        .unwrap();
    assert_eq!(last_text(&vk_mock).await, render::NO_RECIPIENTS);
}

/// The `message` of the last messages.send call, decoded from the form body.
async fn last_text(vk: &MockServer) -> String {
    let requests = vk.received_requests().await.unwrap();
    let body = requests
        .iter()
        .rev()
        .find(|r| r.url.path().contains("messages.send"))
        .map(|r| String::from_utf8_lossy(&r.body).into_owned())
        .unwrap();
    let encoded = body
        .split('&')
        .find_map(|pair| pair.strip_prefix("message="))
        .unwrap()
        .replace('+', " ");
    let mut bytes = Vec::new();
    let mut chars = encoded.bytes();
    while let Some(b) = chars.next() {
        if b == b'%' {
            let hex: String = [chars.next().unwrap(), chars.next().unwrap()]
                .iter()
                .map(|c| *c as char)
                .collect();
            bytes.push(u8::from_str_radix(&hex, 16).unwrap());
        } else {
            bytes.push(b);
        }
    }
    String::from_utf8(bytes).unwrap()
}

fn count(requests: &[Request], method: &str) -> usize {
    requests
        .iter()
        .filter(|r| {
            r.url
                .path()
                .to_lowercase()
                .ends_with(&method.to_lowercase())
        })
        .count()
}

#[tokio::test]
async fn vk_buttons_are_rate_limited_like_telegram_ones() {
    let college = College::start(false).await;
    let tg_mock = telegram_server().await;
    let vk_mock = vk_mock_server().await;
    let app = test_app(&college, &tg_mock, &vk_mock).await;
    let vk = app.vk.clone().unwrap();
    let storage_id = PlatformUser::Vk(42).to_storage_id();
    let a = app.sign_in(storage_id, "user", "password").await.unwrap();
    let view = View::own(&a, NaiveDate::from_ymd_opt(2026, 9, 1).unwrap());
    app.storage.save_view(storage_id, 100, &view).await.unwrap();
    for _ in 0..10 {
        vk::handle_callback(&app, &vk, vk_callback(42, 42, 100, "schedule:next"))
            .await
            .unwrap();
    }
    let requests = vk_mock.received_requests().await.unwrap();
    assert_eq!(count(&requests, "messages.edit"), 8);
}

#[tokio::test]
async fn vk_subgroup_filters_respect_foreign_groups_and_teachers() {
    let college = College::start(false).await;
    let tg_mock = telegram_server().await;
    let vk_mock = vk_mock_server().await;
    let app = test_app(&college, &tg_mock, &vk_mock).await;
    let vk = app.vk.clone().unwrap();
    let storage_id = PlatformUser::Vk(42).to_storage_id();
    let a = app.sign_in(storage_id, "user", "password").await.unwrap();
    let mut foreign = View::own(&a, NaiveDate::from_ymd_opt(2026, 9, 1).unwrap());
    foreign.group = Some(270);
    app.storage
        .save_view(storage_id, 100, &foreign)
        .await
        .unwrap();
    vk::handle_callback(
        &app,
        &vk,
        vk_callback(42, 42, 100, "schedule:subgroup:set:all"),
    )
    .await
    .unwrap();
    assert!(
        app.storage
            .view(storage_id, 100)
            .await
            .unwrap()
            .unwrap()
            .show_all
    );
    // The user's own preference is untouched by filtering group 270.
    assert!(
        !app.storage
            .account(storage_id)
            .await
            .unwrap()
            .unwrap()
            .show_all
    );

    let teacher_college = College::start(true).await;
    let app = test_app(&teacher_college, &tg_mock, &vk_mock).await;
    let vk = app.vk.clone().unwrap();
    let t = app
        .sign_in(storage_id, "teacher", "password")
        .await
        .unwrap();
    let view = View::own(&t, NaiveDate::from_ymd_opt(2026, 9, 1).unwrap());
    app.storage.save_view(storage_id, 101, &view).await.unwrap();
    vk::handle_callback(
        &app,
        &vk,
        vk_callback(42, 42, 101, "schedule:subgroup:right"),
    )
    .await
    .unwrap();
    assert_eq!(app.storage.view(storage_id, 101).await.unwrap(), Some(view));
}

#[tokio::test]
async fn vk_menus_open_while_the_site_is_down() {
    let college = College::start(false).await;
    let tg_mock = telegram_server().await;
    let vk_mock = vk_mock_server().await;
    let app = test_app(&college, &tg_mock, &vk_mock).await;
    let vk = app.vk.clone().unwrap();
    let storage_id = PlatformUser::Vk(42).to_storage_id();
    let a = app.sign_in(storage_id, "user", "password").await.unwrap();
    let view = View::own(&a, NaiveDate::from_ymd_opt(2026, 9, 1).unwrap());
    app.storage.save_view(storage_id, 100, &view).await.unwrap();
    college
        .offline
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let before = college.server.received_requests().await.unwrap().len();
    for data in ["schedule:subgroup:menu", "schedule:week:select"] {
        vk::handle_callback(&app, &vk, vk_callback(42, 42, 100, data))
            .await
            .unwrap();
    }
    assert_eq!(
        college.server.received_requests().await.unwrap().len(),
        before
    );
    assert_eq!(
        app.storage
            .view(storage_id, 100)
            .await
            .unwrap()
            .unwrap()
            .screen,
        Screen::Weeks
    );
}

#[tokio::test]
async fn vk_words_match_regardless_of_case() {
    let college = College::start(false).await;
    let tg_mock = telegram_server().await;
    let vk_mock = vk_mock_server().await;
    let app = test_app(&college, &tg_mock, &vk_mock).await;
    let vk = app.vk.clone().unwrap();
    let storage_id = PlatformUser::Vk(42).to_storage_id();
    app.sign_in(storage_id, "user", "password").await.unwrap();
    let before = college.server.received_requests().await.unwrap().len();
    // "Обновить" refreshes the timetable instead of answering with the help text.
    vk::handle_message(&app, &vk, vk_message(42, 42, "Обновить"))
        .await
        .unwrap();
    assert!(college.server.received_requests().await.unwrap().len() > before);
}

#[tokio::test]
async fn vk_api_calls_are_paced_and_retried_when_rate_limited() {
    let college = College::start(false).await;
    let tg_mock = telegram_server().await;
    let vk_mock = vk_mock_server().await;
    Mock::given(method("POST"))
        .and(path_regex("(?i)/messages.send"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "error": {"error_code": 6, "error_msg": "Too many requests per second"}
        })))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&vk_mock)
        .await;
    let app = test_app(&college, &tg_mock, &vk_mock).await;
    let vk = app.vk.clone().unwrap();
    let started = std::time::Instant::now();
    for _ in 0..5 {
        assert_eq!(vk.send_message(42, "hi", None).await.unwrap(), 100);
    }
    // One retry after a second, and at least 60 ms between any two calls.
    assert!(started.elapsed() >= std::time::Duration::from_millis(1240));
    let requests = vk_mock.received_requests().await.unwrap();
    assert_eq!(count(&requests, "messages.send"), 6);
    let ids: std::collections::HashSet<_> = requests
        .iter()
        .filter_map(|r| {
            String::from_utf8_lossy(&r.body)
                .split('&')
                .find_map(|pair| pair.strip_prefix("random_id=").map(str::to_owned))
        })
        .collect();
    assert!(ids.len() >= 5, "random_id must differ between messages");
}

#[tokio::test]
async fn vk_long_poll_errors_never_reveal_the_key() {
    let college = College::start(false).await;
    let tg_mock = telegram_server().await;
    let vk_mock = vk_mock_server().await;
    let app = test_app(&college, &tg_mock, &vk_mock).await;
    let vk = app.vk.clone().unwrap();
    let server = vk::LongPollServer {
        key: "SECRET-LONG-POLL-KEY".into(),
        server: "https://127.0.0.1:9/whp/1".into(),
        ts: "1".into(),
    };
    let error = vk.poll(&server, 1).await.unwrap_err();
    assert!(
        !format!("{error:#} {error:?}").contains("SECRET"),
        "{error:#}"
    );
    let plain = vk::LongPollServer {
        server: "http://lp.vk.com/whp/1".into(),
        ..server
    };
    assert!(vk.poll(&plain, 1).await.is_err());
}
