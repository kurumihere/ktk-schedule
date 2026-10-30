mod support;
use chrono::NaiveDate;
use ktk_schedule::{
    app::App,
    config::Config,
    model::{PlatformUser, Screen, View},
    vk::{self, MessageEventObject, MessageObject},
};
use serde_json::json;
use std::sync::Arc;
use support::*;
use teloxide::{Bot, adaptors::throttle::Limits, requests::RequesterExt};
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
    let mut config = Config::from_values(|key| match key {
        "BOT_TOKEN" => Some("123456:TEST".into()),
        "CREDENTIALS_SECRET" => Some("x".repeat(32)),
        "DATABASE_PATH" => Some(":memory:".into()),
        "VK_GROUP_TOKEN" => Some("vk_test_token".into()),
        "VK_GROUP_ID" => Some("242576636".into()),
        "VK_API_URL" => Some(format!("{}/", vk_mock.uri())),
        "OWNER_VK_ID" => Some("777".into()),
        _ => None,
    })
    .unwrap();
    config.base_url = college.server.uri().parse().unwrap();
    let bot = Bot::new("123456:TEST")
        .set_api_url(tg_mock.uri().parse().unwrap())
        .throttle(Limits::default());
    Arc::new(App::new(config, bot, "test_bot".into()).await.unwrap())
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
    assert_eq!(account.login, "1000204");
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
                subjects: Vec::new(),
                ..Default::default()
            })
            .collect();

        for teacher in [false, true] {
            for foreign_group in [None, Some(270)] {
                view.group = foreign_group;
                let kb = vk::schedule_keyboard_vk(&days, &view, today, tz, 0, teacher);
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
    let requests = vk_mock.received_requests().await.unwrap();
    let last = requests
        .iter()
        .rev()
        .find(|r| r.url.path().contains("messages.send"))
        .unwrap();
    let body = String::from_utf8_lossy(&last.body);
    assert!(
        body.contains("%D0%9A%D0%BE%D0%BC%D0%B0%D0%BD%D0%B4%D0%B0+%D0%B4%D0%BE%D1%81%D1%82%D1%83%D0%BF%D0%BD%D0%B0")
            || body.contains("администратору")
    );

    // 3. /stats by owner (777) -> success
    vk::handle_message(&app, &vk, vk_message(777, 777, "/stats"))
        .await
        .unwrap();
    let requests = vk_mock.received_requests().await.unwrap();
    let last = requests
        .iter()
        .rev()
        .find(|r| r.url.path().contains("messages.send"))
        .unwrap();
    let body = String::from_utf8_lossy(&last.body);
    assert!(
        body.contains("%D0%A1%D1%82%D0%B0%D1%82%D0%B8%D1%81%D1%82%D0%B8%D0%BA%D0%B0")
            || body.contains("Статистика")
    );

    // 4. /announce by non-owner -> denied
    vk::handle_message(&app, &vk, vk_message(42, 42, "/announce Привет всем"))
        .await
        .unwrap();
    let requests = vk_mock.received_requests().await.unwrap();
    let last = requests
        .iter()
        .rev()
        .find(|r| r.url.path().contains("messages.send"))
        .unwrap();
    let body = String::from_utf8_lossy(&last.body);
    assert!(
        body.contains("%D0%9D%D0%B5%D1%82+%D0%B4%D0%BE%D1%81%D1%82%D1%83%D0%BF%D0%B0")
            || body.contains("доступа")
    );

    // 5. /announce by owner with empty DB -> nobody to send to
    vk::handle_message(&app, &vk, vk_message(777, 777, "/announce Привет всем"))
        .await
        .unwrap();
    let requests = vk_mock.received_requests().await.unwrap();
    let last = requests
        .iter()
        .rev()
        .find(|r| r.url.path().contains("messages.send"))
        .unwrap();
    let body = String::from_utf8_lossy(&last.body);
    assert!(body.contains("%D0%9D%D0%B5%D0%BA%D0%BE%D0%BC%D1%83") || body.contains("Некому"));
}
