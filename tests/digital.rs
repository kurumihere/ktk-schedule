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
    assert_eq!(client.session_cookie, "education_system_auth_session=S1");
    assert_eq!(
        client.expires_at.as_deref(),
        Some("2030-01-01T00:00:00.000Z")
    );
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
