use chrono::NaiveDate;
use ktk_schedule::{
    credentials::Cipher,
    model::{ScheduleDay, Screen, View},
    storage::{Account, Storage},
};

fn account(id: i64) -> Account {
    Account {
        id,
        login: "test".into(),
        password: "пароль".into(),
        group_id: 269,
        personal_subgroup: "left".into(),
        subgroup: "left".into(),
        show_all: false,
        teacher_hash: String::new(),
        notify: false,
    }
}
fn cipher() -> Cipher {
    Cipher::new(&"x".repeat(32)).unwrap()
}

#[tokio::test]
async fn account_settings_and_independent_views_survive_a_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("data/test.sqlite3");
    let store = Storage::open(path.to_str().unwrap(), cipher())
        .await
        .unwrap();
    let a = account(42);
    store.save_account(&a).await.unwrap();
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
    let saved = store.account(42).await.unwrap().unwrap();
    assert_eq!(saved.password, "пароль");
    assert!(saved.notify);
    assert_eq!(saved.subgroup, "right");
    assert_eq!(saved.personal_subgroup, "left");
    assert_eq!(store.view(42, 1).await.unwrap(), Some(first));
    assert_eq!(store.view(42, 2).await.unwrap(), Some(second));
    assert!(store.view(43, 1).await.unwrap().is_none());
    store.close().await;
}

#[tokio::test]
async fn account_change_clears_private_cache_but_keeps_notification_preference() {
    let store = Storage::open(":memory:", cipher()).await.unwrap();
    let mut a = account(42);
    store.save_account(&a).await.unwrap();
    store.set_notify(42, true).await.unwrap();
    store
        .save_schedule(42, "personal", "2026-08-31", &[ScheduleDay::default()])
        .await
        .unwrap();
    a.login = "different".into();
    store.save_account(&a).await.unwrap();
    assert!(store.account(42).await.unwrap().unwrap().notify);
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
        store.save_account(&account(id)).await.unwrap();
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
async fn sent_notifications_are_remembered_across_restarts() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("test.db");
    let store = Storage::open(path.to_str().unwrap(), cipher())
        .await
        .unwrap();
    store.save_account(&account(42)).await.unwrap();
    store.set_notify(42, true).await.unwrap();
    assert_eq!(store.notification_ids("2026-09-01").await.unwrap(), [42]);
    store.mark_notified(42, "2026-09-01").await.unwrap();
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
async fn changing_the_encryption_key_does_not_silently_return_garbage() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("test.db");
    let store = Storage::open(path.to_str().unwrap(), cipher())
        .await
        .unwrap();
    store.save_account(&account(42)).await.unwrap();
    store.close().await;
    let store = Storage::open(
        path.to_str().unwrap(),
        Cipher::new(&"y".repeat(32)).unwrap(),
    )
    .await
    .unwrap();
    assert!(store.account(42).await.is_err());
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
    store.save_account(&a).await.unwrap();
    store.save_account(&account(43)).await.unwrap();
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
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
    let sealed: String = sqlx::query_scalar("SELECT data FROM schedules WHERE telegram_id=42")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(sealed.starts_with("cache:v1:"));
    assert!(!sealed.contains("private-marker"));
    let view: String = sqlx::query_scalar("SELECT data FROM views WHERE telegram_id=42")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(view.starts_with("cache:v1:"));
    assert!(!view.contains("2026-09-01"));
    for (id, scope, week) in [
        (43, "personal", "2026-08-31"),
        (42, "group:269", "2026-08-31"),
        (42, "personal", "2026-09-07"),
    ] {
        sqlx::query("INSERT INTO schedules(telegram_id,scope,week,data) VALUES(?,?,?,?)")
            .bind(id)
            .bind(scope)
            .bind(week)
            .bind(&sealed)
            .execute(&pool)
            .await
            .unwrap();
        assert!(store.schedule(id, scope, week).await.is_err());
    }
    sqlx::query("INSERT INTO views(telegram_id,message_id,data) VALUES(42,2,?)")
        .bind(&view)
        .execute(&pool)
        .await
        .unwrap();
    assert!(store.view(42, 2).await.is_err());
    sqlx::query("UPDATE schedules SET data=? WHERE telegram_id=42 AND scope='personal' AND week='2026-08-31'").bind(serde_json::to_string(&days).unwrap()).execute(&pool).await.unwrap();
    assert!(store.schedule(42, "personal", "2026-08-31").await.is_err());
    pool.close().await;
    store.close().await;
}

#[tokio::test]
async fn plaintext_cache_migration_preserves_accounts_and_only_runs_once() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("legacy.db");
    let pool = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true),
    )
    .await
    .unwrap();
    let mut migrations = sqlx::migrate!();
    migrations.migrations = std::borrow::Cow::Owned(
        migrations
            .iter()
            .filter(|m| m.version == 1)
            .cloned()
            .collect(),
    );
    migrations.run(&pool).await.unwrap();
    sqlx::query("INSERT INTO accounts(telegram_id,login,password,group_id,personal_subgroup,subgroup,notify) VALUES(42,'test',?,269,'left','left',1)").bind(cipher().encrypt(42, "пароль").unwrap()).execute(&pool).await.unwrap();
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
    assert_eq!(a.password, "пароль");
    assert!(a.notify);
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
