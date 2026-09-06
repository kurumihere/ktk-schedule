mod support;
use chrono::NaiveDate;
use ktk_schedule::{
    model::{Screen, View},
    telegram::{handle_callback, handle_message},
};
use serde_json::{Value, json};
use std::sync::{Arc, atomic::Ordering};
use support::*;
use teloxide::types::{CallbackQuery, Message};

fn callback(id: i64, message: i32, data: &str) -> CallbackQuery {
    serde_json::from_value(json!({"id":"callback-test","from":{"id":id,"is_bot":false,"first_name":"Test"},"chat_instance":"TEST","data":data,"message":{"message_id":message,"date":1788202800,"chat":{"id":42,"type":"private","first_name":"Test"},"text":"Schedule"}})).unwrap()
}
fn message(chat: i64, text: &str) -> Message {
    serde_json::from_value(json!({"message_id":9,"date":1788202800,"from":{"id":42,"is_bot":false,"first_name":"Test"},"chat":{"id":chat,"type":if chat>0 {"private"} else {"group"},"title":"Test","first_name":"Test"},"text":text})).unwrap()
}
fn date() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 9, 1).unwrap()
}

#[tokio::test]
async fn refresh_button_fetches_new_data_and_updates_the_existing_message() {
    let college = College::start(false).await;
    let server = telegram_server().await;
    let app = Arc::new(app(&college, &server).await);
    let a = app.sign_in(42, "user", "password").await.unwrap();
    let view = View::own(&a, date());
    app.show(&a, &view, None, false).await.unwrap();
    college.changed.store(true, Ordering::SeqCst);
    handle_callback(app.clone(), callback(42, 100, "schedule:refresh"))
        .await
        .unwrap();
    let requests = server.received_requests().await.unwrap();
    let edit: Value = requests
        .iter()
        .find(|r| r.url.path().to_lowercase().ends_with("/editmessagetext"))
        .unwrap()
        .body_json()
        .unwrap();
    assert_eq!(edit["message_id"], 100);
    assert!(edit["text"].as_str().unwrap().contains("Updated"));
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.url.path().to_lowercase().ends_with("/sendmessage"))
            .count(),
        1
    );
}

#[tokio::test]
async fn changing_weeks_keeps_the_selected_foreign_group_and_other_messages_unchanged() {
    let college = College::start(false).await;
    let server = telegram_server().await;
    let app = Arc::new(app(&college, &server).await);
    let a = app.sign_in(42, "user", "password").await.unwrap();
    let mut first = View::own(&a, date());
    first.group = Some(270);
    let second = View::own(&a, date());
    app.storage.save_view(42, 100, &first).await.unwrap();
    app.storage.save_view(42, 101, &second).await.unwrap();
    handle_callback(app.clone(), callback(42, 100, "schedule:week:next"))
        .await
        .unwrap();
    let updated = app.storage.view(42, 100).await.unwrap().unwrap();
    assert_eq!(updated.group, Some(270));
    assert_eq!(updated.week().to_string(), "2026-09-07");
    assert_eq!(app.storage.view(42, 101).await.unwrap(), Some(second));
    let requests = college.server.received_requests().await.unwrap();
    assert!(
        requests
            .iter()
            .any(|r| r.url.path() == "/v3/ws/groups/schedule"
                && r.url.query_pairs().any(|(k, v)| k == "Group" && v == "270"))
    );
}

#[tokio::test]
async fn buttons_cannot_be_used_by_another_telegram_user() {
    let college = College::start(false).await;
    let server = telegram_server().await;
    let app = Arc::new(app(&college, &server).await);
    handle_callback(app, callback(43, 100, "schedule:download"))
        .await
        .unwrap();
    assert!(college.server.received_requests().await.unwrap().is_empty());
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let answer: Value = requests[0].body_json().unwrap();
    assert_eq!(answer["show_alert"], true);
}

#[tokio::test]
async fn back_cancels_group_input() {
    let college = College::start(false).await;
    let server = telegram_server().await;
    let app = Arc::new(app(&college, &server).await);
    let a = app.sign_in(42, "user", "password").await.unwrap();
    let view = View::own(&a, date());
    app.storage.save_view(42, 100, &view).await.unwrap();
    handle_callback(app.clone(), callback(42, 100, "schedule:group:select"))
        .await
        .unwrap();
    assert_eq!(app.pending_groups.get(&42).await, Some(100));
    handle_callback(app.clone(), callback(42, 100, "schedule:back"))
        .await
        .unwrap();
    assert!(app.pending_groups.get(&42).await.is_none());
    assert_eq!(
        app.storage.view(42, 100).await.unwrap().unwrap().screen,
        Screen::Schedule
    );
}

#[tokio::test]
async fn login_in_a_group_does_not_send_credentials_to_workspace() {
    let college = College::start(false).await;
    let server = telegram_server().await;
    let app = Arc::new(app(&college, &server).await);
    handle_message(app, message(-100, "/login user password"))
        .await
        .unwrap();
    assert!(college.server.received_requests().await.unwrap().is_empty());
    let requests = server.received_requests().await.unwrap();
    let response: Value = requests[0].body_json().unwrap();
    assert_eq!(
        response["text"],
        "Авторизация доступна только в личном чате с ботом."
    );
}

#[tokio::test]
async fn start_keeps_the_help_text_and_commands_for_other_bots_are_ignored() {
    let college = College::start(false).await;
    let server = telegram_server().await;
    let app = Arc::new(app(&college, &server).await);
    handle_message(app.clone(), message(42, "/start@test_bot"))
        .await
        .unwrap();
    handle_message(app, message(42, "/start@different_bot"))
        .await
        .unwrap();
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let response: Value = requests[0].body_json().unwrap();
    assert_eq!(response["text"], ktk_schedule::render::HELP);
}
