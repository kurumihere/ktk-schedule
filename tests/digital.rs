//! ktk.education-system.ru: TanStack Start server functions speaking seroval.
mod support;
use chrono::NaiveDate;
use ktk_schedule::{
    model::{SessionRecord, Week},
    workspace::{DEFAULT_DIGITAL_STUDENT, Workspace, unauthorized},
};
use support::*;
use wiremock::{Mock, ResponseTemplate, matchers::path};

fn week(tz: chrono_tz::Tz) -> Week {
    Week::new(NaiveDate::from_ymd_opt(2026, 9, 1).unwrap(), tz).unwrap()
}

#[tokio::test]
async fn sign_in_escapes_credentials_like_the_website_and_needs_no_discovery() {
    let digital = Digital::start(false, "left").await;
    // Backslash sequences, quotes and `<` are rewritten by the server unless escaped.
    let password = r#"p"a\ss\n<x>"#;
    let client = Workspace::login(&digital.site(), "1000204", password)
        .await
        .unwrap();
    assert_eq!(*digital.passwords.lock().unwrap(), [password]);
    assert_eq!(client.group, 269);
    assert!(client.teacher.is_empty());
    assert_eq!(client.session_id, "sess-1");
    // The session is all that will be kept; it carries the site's cookie, never the password.
    let record = client.session_record().await;
    assert_eq!(record.cookie, "education_system_auth_session=S1");
    assert!(!serde_json::to_string(&record).unwrap().contains(password));
    // Known server function ids work: no page or script crawl at sign-in.
    assert!(digital.requests_to("/").await.is_empty());

    let error = Workspace::login(&digital.site(), "1000204", "wrong")
        .await
        .err()
        .unwrap();
    assert_eq!(error.to_string(), "Неверный логин или пароль.");
}

#[tokio::test]
async fn weeks_follow_the_configured_timezone_and_text_is_unescaped() {
    let digital = Digital::start(false, "left").await;
    let client = Workspace::login(&digital.site(), "1000204", "secret")
        .await
        .unwrap();
    // Monday 06:00 at UTC+14 is still Sunday in Yekaterinburg: the old hardcoded zone
    // asked the site for the previous week.
    for tz in [
        chrono_tz::Pacific::Kiritimati,
        chrono_tz::Asia::Yekaterinburg,
    ] {
        let days = client.schedule(269, week(tz), "personal").await.unwrap();
        assert_eq!(days[0].date, "2026-08-31");
        assert_eq!(days[0].subjects[0].discipline, "МДК \"Веб\" <1>");
        assert_eq!(days[0].subjects[0].teacher, "Иванова Анна Петровна");
    }
    let sent: Vec<_> = digital
        .requests_to(DEFAULT_DIGITAL_STUDENT)
        .await
        .iter()
        .map(|r| server_fn_data(r)["weekStart"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(sent, ["2026-08-31", "2026-08-31"]);
    let refs = client.references().await;
    assert_eq!(refs.halls[&389].number, "18А");
    assert_eq!(refs.presets[&55].call_set[0].duration, 90);
}

#[tokio::test]
async fn expired_sessions_are_reported_as_unauthorized() {
    let digital = Digital::start(false, "left").await;
    let client = Workspace::login(&digital.site(), "1000204", "secret")
        .await
        .unwrap();
    digital.dead.lock().unwrap().insert("S1".into());
    let error = client
        .schedule(269, week(chrono_tz::Asia::Yekaterinburg), "personal")
        .await
        .unwrap_err();
    // An in-band `authRequired` must trigger a fresh sign-in, not a stale fallback.
    assert!(unauthorized(&error), "{error:#}");
}

#[tokio::test]
async fn restored_sessions_send_the_saved_cookie_and_follow_rotation() {
    let digital = Digital::start(false, "left").await;
    let site = digital.site();
    let record = SessionRecord {
        cookie: "education_system_auth_session=S7".into(),
        session_id: "sess-7".into(),
        group: 269,
        ..Default::default()
    };
    let client = Workspace::from_session(&site, &record).unwrap();
    let tz = chrono_tz::Asia::Yekaterinburg;
    // The site may refresh a sliding session; the bot must use the new cookie afterwards.
    let rotation = Mock::given(path(DEFAULT_DIGITAL_STUDENT))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("set-cookie", "education_system_auth_session=S8; Path=/")
                .set_body_json(seroval(&digital_week(
                    "2026-08-31",
                    "Rotated",
                    "left",
                    "269",
                ))),
        )
        .up_to_n_times(1)
        .with_priority(1)
        .mount_as_scoped(&digital.server)
        .await;
    client.schedule(269, week(tz), "personal").await.unwrap();
    drop(rotation);
    digital.dead.lock().unwrap().insert("S7".into());
    client.schedule(269, week(tz), "personal").await.unwrap();
    let cookies: Vec<_> = digital
        .requests_to(DEFAULT_DIGITAL_STUDENT)
        .await
        .iter()
        .map(|r| {
            r.headers
                .get("cookie")
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(
        cookies,
        [
            "education_system_auth_session=S7",
            "education_system_auth_session=S8"
        ]
    );
    assert!(Workspace::from_session(&site, &SessionRecord::default()).is_err());
}

#[tokio::test]
async fn moved_server_functions_are_rediscovered_once_for_every_session() {
    let digital = Digital::start(false, "left").await;
    let site = digital.site();
    let first = Workspace::login(&site, "1000204", "secret").await.unwrap();
    let second = Workspace::login(&site, "1000204", "secret").await.unwrap();
    let moved = "/_serverFn/abcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd";
    Mock::given(path(DEFAULT_DIGITAL_STUDENT))
        .respond_with(ResponseTemplate::new(404))
        .with_priority(1)
        .mount(&digital.server)
        .await;
    Mock::given(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<script type=\"module\" src=\"/static/assets/app.js\"></script>"),
        )
        .mount(&digital.server)
        .await;
    Mock::given(path("/static/assets/app.js"))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!(
            r#"var s=createServerFn({{method:"GET"}}).validator(e=>({{weekStart:e.weekStart}})).handler(r("{moved}"));"#
        )))
        .mount(&digital.server)
        .await;
    Mock::given(path(moved))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(seroval(&digital_week(
                "2026-08-31",
                "Moved",
                "left",
                "269",
            ))),
        )
        .mount(&digital.server)
        .await;
    let tz = chrono_tz::Asia::Yekaterinburg;
    for client in [&first, &second] {
        let days = client.schedule(269, week(tz), "personal").await.unwrap();
        assert_eq!(days[0].subjects[0].discipline, "Moved");
    }
    // One crawl serves every session of the bot.
    assert_eq!(digital.requests_to("/").await.len(), 1);
    assert_eq!(digital.requests_to("/static/assets/app.js").await.len(), 1);
    // Afterwards the new address is used directly.
    first.schedule(269, week(tz), "personal").await.unwrap();
    assert_eq!(digital.requests_to(DEFAULT_DIGITAL_STUDENT).await.len(), 2);
}

#[tokio::test]
async fn revoking_ends_the_session_on_the_site() {
    let digital = Digital::start(false, "left").await;
    let client = Workspace::login(&digital.site(), "1000204", "secret")
        .await
        .unwrap();
    client.revoke_session().await.unwrap();
    assert_eq!(*digital.revoked.lock().unwrap(), ["sess-1"]);
}

mod app {
    use super::*;
    use ktk_schedule::{
        app::login_required,
        model::View,
        render,
        telegram::handle_message,
        workspace::{DEFAULT_DIGITAL_LOGOUT, DEFAULT_DIGITAL_SIGN_IN},
    };
    use serde_json::{Value, json};
    use std::sync::Arc;
    use teloxide::types::Message;

    fn today_view(app: &ktk_schedule::app::App, account: &ktk_schedule::storage::Account) -> View {
        View::own(account, app.now().date_naive())
    }

    fn message(text: &str) -> Message {
        serde_json::from_value(json!({"message_id":9,"date":1788202800,"from":{"id":42,"is_bot":false,"first_name":"Test"},"chat":{"id":42,"type":"private","first_name":"Test"},"text":text})).unwrap()
    }

    async fn sent_texts(telegram: &wiremock::MockServer) -> Vec<String> {
        telegram
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.url.path().to_lowercase().ends_with("/sendmessage"))
            .map(|r| {
                r.body_json::<Value>().unwrap()["text"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect()
    }

    #[tokio::test]
    async fn sign_in_stores_no_password_and_detects_the_subgroup() {
        let digital = Digital::start(false, "right").await;
        let telegram = telegram_server().await;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("bot.sqlite3");
        let app = digital_app(&digital, &telegram, path.to_str().unwrap()).await;
        let password = "Sup3r-Secret-Passw0rd";
        let account = app.sign_in(42, "1000204", password).await.unwrap();
        // The personal timetable lists only right-subgroup split lessons.
        assert_eq!(account.personal_subgroup, "right");
        assert_eq!(account.subgroup, "right");
        // Detection fetched this week already; the first /schedule reuses it.
        let fetched = digital.requests_to(DEFAULT_DIGITAL_STUDENT).await.len();
        app.load(&account, &today_view(&app, &account), false)
            .await
            .unwrap();
        assert_eq!(
            digital.requests_to(DEFAULT_DIGITAL_STUDENT).await.len(),
            fetched
        );
        app.storage.close().await;
        for file in [path.clone(), path.with_extension("sqlite3-wal")] {
            if let Ok(bytes) = std::fs::read(&file) {
                for secret in [password, "1000204"] {
                    assert!(
                        !bytes.windows(secret.len()).any(|w| w == secret.as_bytes()),
                        "{secret} found in {}",
                        file.display()
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn a_rejected_session_asks_for_a_new_sign_in_instead_of_reusing_a_password() {
        let digital = Digital::start(false, "left").await;
        let telegram = telegram_server().await;
        let app = Arc::new(digital_app(&digital, &telegram, ":memory:").await);
        let account = app.sign_in(42, "1000204", "secret").await.unwrap();
        let view = today_view(&app, &account);
        app.load(&account, &view, true).await.unwrap();
        digital.dead.lock().unwrap().insert("S1".into());
        let error = app.load(&account, &view, true).await.err().unwrap();
        assert!(login_required(&error), "{error:#}");
        assert_eq!(
            digital.sign_ins.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert!(app.storage.session(42).await.unwrap().is_none());
        // From now on the bot knows without asking the site.
        let before = digital.server.received_requests().await.unwrap().len();
        assert!(login_required(
            &app.load(&account, &view, true).await.err().unwrap()
        ));
        assert_eq!(
            digital.server.received_requests().await.unwrap().len(),
            before
        );
        handle_message(app.clone(), message("/schedule"))
            .await
            .unwrap();
        assert_eq!(
            sent_texts(&telegram).await.last().unwrap(),
            render::LOGIN_AGAIN
        );
        // Signing in again restores everything.
        let account = app.sign_in(42, "1000204", "secret").await.unwrap();
        app.load(&account, &view, true).await.unwrap();
    }

    #[tokio::test]
    async fn a_refused_group_timetable_does_not_end_a_live_session() {
        let digital = Digital::start(false, "left").await;
        let telegram = telegram_server().await;
        let app = digital_app(&digital, &telegram, ":memory:").await;
        let account = app.sign_in(42, "1000204", "secret").await.unwrap();
        Mock::given(path(ktk_schedule::workspace::DEFAULT_DIGITAL_GROUP))
            .respond_with(ResponseTemplate::new(403))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&digital.server)
            .await;
        let mut view = today_view(&app, &account);
        view.group = Some(270);
        let error = app.load(&account, &view, false).await.err().unwrap();
        assert!(!login_required(&error), "{error:#}");
        // The personal timetable still loads, so the session stays.
        assert!(app.storage.session(42).await.unwrap().is_some());
        app.load(&account, &view, false).await.unwrap();
    }

    #[tokio::test]
    async fn restarts_restore_the_session_without_signing_in() {
        let digital = Digital::start(false, "left").await;
        let telegram = telegram_server().await;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("bot.sqlite3");
        let first = digital_app(&digital, &telegram, path.to_str().unwrap()).await;
        let account = first.sign_in(42, "1000204", "secret").await.unwrap();
        first.storage.close().await;
        drop(first);
        let second = digital_app(&digital, &telegram, path.to_str().unwrap()).await;
        let loaded = second
            .load(&account, &today_view(&second, &account), true)
            .await
            .unwrap();
        assert!(!loaded.stale);
        assert_eq!(digital.requests_to(DEFAULT_DIGITAL_SIGN_IN).await.len(), 1);
        let cookie = digital.requests_to(DEFAULT_DIGITAL_STUDENT).await;
        assert_eq!(
            cookie.last().unwrap().headers.get("cookie").unwrap(),
            "education_system_auth_session=S1"
        );
    }

    #[tokio::test]
    async fn signing_in_again_ends_the_old_session_only_after_success() {
        let digital = Digital::start(false, "left").await;
        let telegram = telegram_server().await;
        let app = digital_app(&digital, &telegram, ":memory:").await;
        app.sign_in(42, "1000204", "secret").await.unwrap();
        // A typo must not cost the user their working session.
        assert!(app.sign_in(42, "1000204", "wrong").await.is_err());
        assert!(digital.revoked.lock().unwrap().is_empty());
        assert!(app.storage.session(42).await.unwrap().is_some());
        app.sign_in(42, "1000204", "secret").await.unwrap();
        assert_eq!(*digital.revoked.lock().unwrap(), ["sess-1"]);
        app.sign_out(42).await.unwrap();
        assert_eq!(*digital.revoked.lock().unwrap(), ["sess-1", "sess-2"]);
        assert_eq!(digital.requests_to(DEFAULT_DIGITAL_LOGOUT).await.len(), 2);
        assert!(app.storage.account(42).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn a_session_that_ended_overnight_is_announced_once() {
        let digital = Digital::start(false, "left").await;
        let telegram = telegram_server().await;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("bot.sqlite3");
        let evening = digital_app(&digital, &telegram, path.to_str().unwrap()).await;
        evening.sign_in(42, "1000204", "secret").await.unwrap();
        evening.storage.set_notify(42, true).await.unwrap();
        evening.storage.close().await;
        drop(evening);
        digital.dead.lock().unwrap().insert("S1".into());
        // The next morning (a fresh process: nothing cached in memory).
        let app = digital_app(&digital, &telegram, path.to_str().unwrap()).await;
        let today = app.now().date_naive();
        assert_eq!(
            app.storage
                .notification_ids(&today.to_string())
                .await
                .unwrap(),
            [42]
        );
        app.notify_one(42, today).await.unwrap();
        assert_eq!(sent_texts(&telegram).await, [render::LOGIN_AGAIN]);
        let tomorrow = today.succ_opt().unwrap();
        assert!(
            app.storage
                .notification_ids(&tomorrow.to_string())
                .await
                .unwrap()
                .is_empty()
        );
    }
}
