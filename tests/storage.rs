use chrono::NaiveDate;
use ktk_schedule::{
    credentials::Cipher,
    model::{Call, Hall, Preset, ReferenceData, ScheduleDay, Screen, SessionRecord, View},
    storage::{Account, Storage},
};
use sqlx::{SqlitePool, sqlite::SqliteConnectOptions};

fn account(id: i64) -> Account {
    Account {
        id,
        group_id: 269,
        personal_subgroup: "left".into(),
        subgroup: "left".into(),
        show_all: false,
        teacher_hash: String::new(),
        notify: false,
    }
}
fn session(marker: &str) -> SessionRecord {
    SessionRecord {
        cookie: format!("education_system_auth_session={marker}"),
        session_id: format!("sess-{marker}"),
        group: 269,
        subgroup: "left".into(),
        branch: "branch-uuid".into(),
        personal_endpoint: "/personal".into(),
        group_endpoint: "/group".into(),
        ..Default::default()
    }
}
fn cipher() -> Cipher {
    Cipher::new(&"x".repeat(32)).unwrap()
}
async fn raw(path: &std::path::Path) -> SqlitePool {
    SqlitePool::connect_with(SqliteConnectOptions::new().filename(path))
        .await
        .unwrap()
}

#[tokio::test]
async fn account_settings_and_independent_views_survive_a_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("data/test.sqlite3");
    let store = Storage::open(path.to_str().unwrap(), cipher())
        .await
        .unwrap();
    let a = account(42);
    store.save_account(&a, &session("A")).await.unwrap();
    store.set_notify(42, true).await.unwrap();
    store.set_subgroup(42, "right", true).await.unwrap();
    let mut first = View::own(&a, NaiveDate::from_ymd_opt(2026, 9, 1).unwrap());
    first.group = Some(270);
    let mut second = first.clone();
    second.screen = Screen::Weeks;
    store.save_view(42, 1, &first).await.unwrap();
    store.save_view(42, 2, &second).await.unwrap();
    store.close().await;
    let store = Storage::open(path.to_str().unwrap(), cipher())
        .await
        .unwrap();
    store.check_secret().await.unwrap();
    let saved = store.account(42).await.unwrap().unwrap();
    assert!(saved.notify);
    assert_eq!(saved.subgroup, "right");
    assert_eq!(saved.personal_subgroup, "left");
    assert_eq!(
        store.session(42).await.unwrap().unwrap().cookie,
        "education_system_auth_session=A"
    );
    assert_eq!(store.view(42, 1).await.unwrap(), Some(first));
    assert_eq!(store.view(42, 2).await.unwrap(), Some(second));
    assert!(store.view(43, 1).await.unwrap().is_none());
    store.close().await;
}

#[tokio::test]
async fn signing_in_again_clears_private_cache_but_keeps_notification_preference() {
    let store = Storage::open(":memory:", cipher()).await.unwrap();
    let a = account(42);
    store.save_account(&a, &session("A")).await.unwrap();
    store.set_notify(42, true).await.unwrap();
    store
        .save_schedule(42, "personal", "2026-08-31", &[ScheduleDay::default()])
        .await
        .unwrap();
    store.save_account(&a, &session("B")).await.unwrap();
    assert!(store.account(42).await.unwrap().unwrap().notify);
    assert_eq!(
        store.session(42).await.unwrap().unwrap().session_id,
        "sess-B"
    );
    assert!(
        store
            .schedule(42, "personal", "2026-08-31")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn private_and_group_caches_are_separate_for_every_account() {
    let store = Storage::open(":memory:", cipher()).await.unwrap();
    for id in [42, 43] {
        store
            .save_account(&account(id), &session("A"))
            .await
            .unwrap();
    }
    let days = vec![ScheduleDay {
        date: "2026-09-01".into(),
        ..Default::default()
    }];
    store
        .save_schedule(42, "personal", "2026-08-31", &days)
        .await
        .unwrap();
    assert_eq!(
        store.schedule(42, "personal", "2026-08-31").await.unwrap(),
        Some(days)
    );
    assert!(
        store
            .schedule(43, "personal", "2026-08-31")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .schedule(42, "group:269", "2026-08-31")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn reference_cache_survives_a_restart_and_clears_on_a_new_sign_in() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("refs.db");
    let store = Storage::open(path.to_str().unwrap(), cipher())
        .await
        .unwrap();
    store
        .save_account(&account(42), &session("A"))
        .await
        .unwrap();
    assert!(store.references(42).await.unwrap().is_none());
    let mut refs = ReferenceData::default();
    refs.halls.insert(
        324,
        Hall {
            id: 324,
            number: "324".into(),
            ..Default::default()
        },
    );
    refs.presets.insert(
        55,
        Preset {
            id: 55,
            begin: "0000-01-01T09:10:00Z".into(),
            call_set: vec![Call {
                r#break: 10,
                duration: 45,
                pair_number: 0,
            }],
        },
    );
    store.save_references(42, &refs).await.unwrap();
    store.close().await;
    let store = Storage::open(path.to_str().unwrap(), cipher())
        .await
        .unwrap();
    let cached = store.references(42).await.unwrap().unwrap();
    assert_eq!(cached.halls[&324].number, "324");
    assert_eq!(cached.presets[&55].call_set.len(), 1);
    store
        .save_account(&account(42), &session("B"))
        .await
        .unwrap();
    assert!(store.references(42).await.unwrap().is_none());
    store.close().await;
}

#[tokio::test]
async fn sessions_survive_a_restart_and_vanish_with_the_account() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("sess.db");
    let store = Storage::open(path.to_str().unwrap(), cipher())
        .await
        .unwrap();
    store
        .save_account(&account(42), &session("A"))
        .await
        .unwrap();
    let mut rotated = session("A");
    rotated.cookie = "education_system_auth_session=A2; theme=dark".into();
    store.save_session(42, &rotated).await.unwrap();
    store.close().await;

    let store = Storage::open(path.to_str().unwrap(), cipher())
        .await
        .unwrap();
    let cached = store.session(42).await.unwrap().unwrap();
    assert_eq!(cached.cookie, rotated.cookie);
    assert_eq!(cached.session_id, "sess-A");
    assert_eq!(cached.group, 269);
    // Records written by older versions still load.
    let legacy: SessionRecord = serde_json::from_str(
        r#"{"cookie":"c=1","session_id":"s","group":1,"subgroup":"","teacher":"","branch":"","personal_endpoint":"","group_endpoint":"","expires_at":null}"#,
    )
    .unwrap();
    assert_eq!(legacy.cookie, "c=1");
    store.delete_session(42).await.unwrap();
    assert!(store.session(42).await.unwrap().is_none());
    store.save_session(42, &session("C")).await.unwrap();
    store.delete_account(42).await.unwrap();
    assert!(store.session(42).await.unwrap().is_none());
    store.close().await;
}

#[tokio::test]
async fn notifications_are_remembered_and_need_a_live_session() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("test.db");
    let store = Storage::open(path.to_str().unwrap(), cipher())
        .await
        .unwrap();
    for id in [42, 43] {
        store
            .save_account(&account(id), &session("A"))
            .await
            .unwrap();
        store.set_notify(id, true).await.unwrap();
    }
    assert_eq!(
        store.notification_ids("2026-09-01").await.unwrap(),
        [42, 43]
    );
    store.mark_notified(42, "2026-09-01").await.unwrap();
    // Without a session there is nothing to load: only a new sign-in brings it back.
    store.delete_session(43).await.unwrap();
    store.close().await;
    let store = Storage::open(path.to_str().unwrap(), cipher())
        .await
        .unwrap();
    assert!(
        store
            .notification_ids("2026-09-01")
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(store.notification_ids("2026-09-02").await.unwrap(), [42]);
    store.close().await;
}

#[tokio::test]
async fn changing_the_encryption_key_is_detected_instead_of_returning_garbage() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("test.db");
    let store = Storage::open(path.to_str().unwrap(), cipher())
        .await
        .unwrap();
    store
        .save_account(&account(42), &session("A"))
        .await
        .unwrap();
    store.close().await;
    let store = Storage::open(
        path.to_str().unwrap(),
        Cipher::new(&"y".repeat(32)).unwrap(),
    )
    .await
    .unwrap();
    assert!(store.session(42).await.is_err());
    let error = store.check_secret().await.unwrap_err();
    assert!(error.to_string().contains("CREDENTIALS_SECRET"), "{error}");
    store.close().await;
}

#[tokio::test]
async fn cache_payloads_are_encrypted_and_cannot_be_swapped_between_rows() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("cache.db");
    let store = Storage::open(path.to_str().unwrap(), cipher())
        .await
        .unwrap();
    let a = account(42);
    store.save_account(&a, &session("A")).await.unwrap();
    store
        .save_account(&account(43), &session("B"))
        .await
        .unwrap();
    let days = vec![ScheduleDay {
        date: "private-marker-2026".into(),
        ..Default::default()
    }];
    store
        .save_schedule(42, "personal", "2026-08-31", &days)
        .await
        .unwrap();
    store
        .save_view(
            42,
            1,
            &View::own(&a, NaiveDate::from_ymd_opt(2026, 9, 1).unwrap()),
        )
        .await
        .unwrap();
    let pool = raw(&path).await;
    let sealed: String = sqlx::query_scalar("SELECT data FROM schedules WHERE user_id=42")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(sealed.starts_with("cache:v1:"));
    assert!(!sealed.contains("private-marker"));
    let view: String = sqlx::query_scalar("SELECT data FROM views WHERE user_id=42")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(view.starts_with("cache:v1:"));
    assert!(!view.contains("2026-09-01"));
    let cookie: String = sqlx::query_scalar("SELECT data FROM sessions WHERE user_id=42")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!cookie.contains("education_system_auth_session"));
    for (id, scope, week) in [
        (43, "personal", "2026-08-31"),
        (42, "group:269", "2026-08-31"),
        (42, "personal", "2026-09-07"),
    ] {
        sqlx::query("INSERT INTO schedules(user_id,scope,week,data) VALUES(?,?,?,?)")
            .bind(id)
            .bind(scope)
            .bind(week)
            .bind(&sealed)
            .execute(&pool)
            .await
            .unwrap();
        assert!(store.schedule(id, scope, week).await.is_err());
    }
    sqlx::query("INSERT INTO views(user_id,message_id,data) VALUES(42,2,?)")
        .bind(&view)
        .execute(&pool)
        .await
        .unwrap();
    assert!(store.view(42, 2).await.is_err());
    sqlx::query("UPDATE sessions SET data=? WHERE user_id=43")
        .bind(&cookie)
        .execute(&pool)
        .await
        .unwrap();
    assert!(store.session(43).await.is_err());
    sqlx::query(
        "UPDATE schedules SET data=? WHERE user_id=42 AND scope='personal' AND week='2026-08-31'",
    )
    .bind(serde_json::to_string(&days).unwrap())
    .execute(&pool)
    .await
    .unwrap();
    assert!(store.schedule(42, "personal", "2026-08-31").await.is_err());
    pool.close().await;
    store.close().await;
}

/// Build a database exactly as an older release left it: migrations up to `version`.
async fn database_at(path: &std::path::Path, version: i64) -> SqlitePool {
    let pool = SqlitePool::connect_with(
        SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true),
    )
    .await
    .unwrap();
    let mut migrations = sqlx::migrate!();
    migrations.migrations = std::borrow::Cow::Owned(
        migrations
            .iter()
            .filter(|m| m.version <= version)
            .cloned()
            .collect(),
    );
    migrations.run(&pool).await.unwrap();
    pool
}

#[tokio::test]
async fn plaintext_cache_migration_preserves_accounts_and_only_runs_once() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("legacy.db");
    let pool = database_at(&path, 1).await;
    sqlx::query("INSERT INTO accounts(telegram_id,login,password,group_id,personal_subgroup,subgroup,notify) VALUES(42,'test','v2:x',269,'left','left',1)").execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO schedules(telegram_id,scope,week,data) VALUES(42,'personal','2026-08-31','[]')").execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO views(telegram_id,message_id,data) VALUES(42,1,'{}')")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    let store = Storage::open(path.to_str().unwrap(), cipher())
        .await
        .unwrap();
    let a = store.account(42).await.unwrap().unwrap();
    assert!(a.notify);
    assert_eq!(a.group_id, 269);
    assert!(
        store
            .schedule(42, "personal", "2026-08-31")
            .await
            .unwrap()
            .is_none()
    );
    assert!(store.view(42, 1).await.unwrap().is_none());
    let days = vec![ScheduleDay::default()];
    store
        .save_schedule(42, "personal", "2026-08-31", &days)
        .await
        .unwrap();
    store.close().await;
    let store = Storage::open(path.to_str().unwrap(), cipher())
        .await
        .unwrap();
    assert_eq!(
        store.schedule(42, "personal", "2026-08-31").await.unwrap(),
        Some(days)
    );
    store.close().await;
}

#[tokio::test]
async fn upgrading_drops_every_stored_password_and_keeps_sessions_and_caches() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("v4.db");
    let pool = database_at(&path, 4).await;
    let cipher = cipher();
    // A real v2 password ciphertext ("пароль") and a recognizable login.
    let password = "v2:BwcHBwcHBwcHBwcHTxoRFaKs0mRPf3uM9BmjUs8X9aMjghx+JvQO5w";
    for (id, notify) in [(42_i64, 1), (1_000_000_000_000_000_007, 0)] {
        sqlx::query("INSERT INTO accounts(telegram_id,login,password,group_id,personal_subgroup,subgroup,show_all,teacher_hash,notify,notified_date) VALUES(?,'login-marker-1000204',?,269,'left','right',1,'',?,'2026-09-01')")
            .bind(id).bind(password).bind(notify).execute(&pool).await.unwrap();
    }
    let record = serde_json::to_string(&session("A")).unwrap();
    let sealed = |context: &str, value: &str| cipher.encrypt_cache(context, value).unwrap();
    sqlx::query("INSERT INTO session_cache(telegram_id,data) VALUES(42,?)")
        .bind(sealed(r#"["sessions",42]"#, &record))
        .execute(&pool)
        .await
        .unwrap();
    let days = vec![ScheduleDay {
        date: "2026-09-01".into(),
        ..Default::default()
    }];
    sqlx::query(
        "INSERT INTO schedules(telegram_id,scope,week,data) VALUES(42,'personal','2026-08-31',?)",
    )
    .bind(sealed(
        r#"["schedules",42,"personal","2026-08-31"]"#,
        &serde_json::to_string(&days).unwrap(),
    ))
    .execute(&pool)
    .await
    .unwrap();
    let view = View::own(&account(42), NaiveDate::from_ymd_opt(2026, 9, 1).unwrap());
    sqlx::query("INSERT INTO views(telegram_id,message_id,data) VALUES(42,7,?)")
        .bind(sealed(
            r#"["views",42,7]"#,
            &serde_json::to_string(&view).unwrap(),
        ))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO reference_cache(telegram_id,data) VALUES(42,?)")
        .bind(sealed(
            r#"["references",42]"#,
            &serde_json::to_string(&ReferenceData::default()).unwrap(),
        ))
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;

    let store = Storage::open(path.to_str().unwrap(), self::cipher())
        .await
        .unwrap();
    store.check_secret().await.unwrap();
    let a = store.account(42).await.unwrap().unwrap();
    assert!(a.notify && a.show_all);
    assert_eq!((a.group_id, a.subgroup.as_str()), (269, "right"));
    let vk = store
        .account(1_000_000_000_000_000_007)
        .await
        .unwrap()
        .unwrap();
    assert!(!vk.notify);
    assert_eq!(
        store.session(42).await.unwrap().unwrap().session_id,
        "sess-A"
    );
    assert!(
        store
            .session(1_000_000_000_000_000_007)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store.schedule(42, "personal", "2026-08-31").await.unwrap(),
        Some(days)
    );
    assert_eq!(store.view(42, 7).await.unwrap(), Some(view));
    assert!(store.references(42).await.unwrap().is_some());
    // Already notified today: the upgrade must not resend the morning message.
    assert!(
        store
            .notification_ids("2026-09-01")
            .await
            .unwrap()
            .is_empty()
    );
    store.close().await;

    let pool = raw(&path).await;
    let columns: Vec<String> = sqlx::query_scalar("SELECT name FROM pragma_table_info('accounts')")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert!(
        !columns.iter().any(|c| c == "password" || c == "login"),
        "{columns:?}"
    );
    let strict: i64 =
        sqlx::query_scalar("SELECT strict FROM pragma_table_list WHERE name = 'accounts'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(strict, 1);
    pool.close().await;
    // Not a trace of the old credentials in the file or its write-ahead log.
    for file in [path.clone(), path.with_extension("db-wal")] {
        if let Ok(bytes) = std::fs::read(&file) {
            for marker in ["TxoRFaKs0mRPf3uM", "login-marker-1000204"] {
                assert!(
                    !bytes.windows(marker.len()).any(|w| w == marker.as_bytes()),
                    "{marker} survived in {}",
                    file.display()
                );
            }
        }
    }
}
