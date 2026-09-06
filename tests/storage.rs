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
