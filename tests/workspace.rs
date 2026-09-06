mod support;
use chrono::NaiveDate;
use chrono_tz::Asia::Yekaterinburg as TZ;
use ktk_schedule::{
    model::{View, week_millis},
    workspace::{Candidates, script_urls},
};
use std::sync::atomic::Ordering;
use support::*;
use wiremock::{Mock, ResponseTemplate, matchers::path};

fn week() -> i64 {
    week_millis(NaiveDate::from_ymd_opt(2026, 9, 1).unwrap(), TZ).unwrap()
}

#[tokio::test]
async fn discovery_separates_personal_grades_from_other_groups_and_reuses_scripts() {
    let college = College::start(false).await;
    let client = college.login().await;
    assert_eq!(client.group, 269);
    assert!(client.teacher.is_empty());
    let personal = client.schedule(269, week(), "personal").await.unwrap();
    let group = client.schedule(270, week(), "group:270").await.unwrap();
    assert_eq!(personal[0].subjects[0].appraisal, 5);
    assert_eq!(group[0].subjects[0].discipline, "Group 270");
    assert_eq!(group[0].subjects[0].appraisal, 0);
    let refs = client.references().await;
    assert_eq!(refs.halls[&1].number, "301");
    assert_eq!(refs.presets[&1].call_set[0].duration, 90);
    let requests = college.server.received_requests().await.unwrap();
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.url.path() == "/assets/main.js")
            .count(),
        1
    );
    assert!(
        requests
            .iter()
            .filter(|r| r.url.path().starts_with("/v3/"))
            .all(|r| r
                .headers
                .get("cookie")
                .is_some_and(|c| c.to_str().unwrap().contains("session=TEST")))
    );
}

#[tokio::test]
async fn teachers_can_open_both_personal_and_group_schedules() {
    let college = College::start(true).await;
    let client = college.login().await;
    assert_eq!(client.teacher, "TEACHER");
    assert_eq!(
        client
            .schedule(269, week(), "teacher:TEACHER")
            .await
            .unwrap()[0]
            .subjects[0]
            .discipline,
        "Personal"
    );
    assert_eq!(
        client.schedule(270, week(), "group:270").await.unwrap()[0].subjects[0].discipline,
        "Group 270"
    );
}

#[tokio::test]
async fn files_and_homework_are_discovered_and_downloaded() {
    let college = College::start(false).await;
    let client = college.login().await;
    client.refresh(269, week(), false).await.unwrap();
    assert_eq!(client.document(10).await.unwrap().caption, "task.pdf");
    assert_eq!(client.submission(7).await.unwrap(), 10);
    let (bytes, name) = client.download(10).await.unwrap();
    assert_eq!(tokio::fs::read(&bytes).await.unwrap(), b"%PDF-test");
    assert_eq!(name, "task.pdf");
}

#[tokio::test]
async fn refresh_reloads_data_and_offline_fallback_remains_private() {
    let college = College::start(false).await;
    let telegram = telegram_server().await;
    let app = app(&college, &telegram).await;
    let account = app.sign_in(42, "user", "password").await.unwrap();
    let view = View::own(&account, NaiveDate::from_ymd_opt(2026, 9, 1).unwrap());
    let first = app.load(&account, &view, false).await.unwrap();
    college.changed.store(true, Ordering::SeqCst);
    let cached = app.load(&account, &view, false).await.unwrap();
    assert!(std::sync::Arc::ptr_eq(&first.days, &cached.days));
    let refreshed = app.load(&account, &view, true).await.unwrap();
    assert_eq!(refreshed.days[0].subjects[0].discipline, "Updated");
    college.offline.store(true, Ordering::SeqCst);
    let saved = app.load(&account, &view, true).await.unwrap();
    assert!(saved.stale);
    assert_eq!(saved.days[0].subjects[0].discipline, "Updated");
    assert!(
        app.storage
            .schedule(43, "personal", &view.week().to_string())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn concurrent_loads_share_one_request_and_one_schedule_allocation() {
    let college = College::start(false).await;
    let telegram = telegram_server().await;
    let app = app(&college, &telegram).await;
    let account = app.sign_in(42, "user", "password").await.unwrap();
    app.session(&account)
        .await
        .unwrap()
        .client
        .refresh(269, week(), false)
        .await
        .unwrap();
    let count_before = college
        .server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == "/v3/ws/personal/schedule")
        .count();
    let view = View::own(&account, NaiveDate::from_ymd_opt(2026, 9, 1).unwrap());
    let results = futures::future::join_all((0..8).map(|_| app.load(&account, &view, false))).await;
    let first = results[0].as_ref().unwrap();
    for result in &results {
        assert!(std::sync::Arc::ptr_eq(
            &first.days,
            &result.as_ref().unwrap().days
        ));
    }
    let count_after = college
        .server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == "/v3/ws/personal/schedule")
        .count();
    assert_eq!(count_after - count_before, 1);
}

#[test]
fn discovery_ignores_external_scripts_and_extracts_versioned_endpoints() {
    let urls = script_urls(
        "<script src='/main.js'></script><script src='https://foreign.test/app.js'></script> './chunk.js'",
        &"https://college.test/".parse().unwrap(),
    );
    assert_eq!(urls.len(), 2);
    let mut c = Candidates::default();
    c.add(
        "'/v3/ws/abc/schedule';'/v3/ws/files/open?ID=1';'/v3/ws/hw/homework/check';Branch%3Dmain",
    );
    assert_eq!(c.schedules, ["/v3/ws/abc/schedule"]);
    assert_eq!(c.files, ["files"]);
    assert_eq!(c.homework, ["hw"]);
    assert_eq!(c.branches, ["main"]);
    let mut variables = Candidates::default();
    variables.add("n.GetCurrentBranch=je; this.storage.Branch=e; e.Branch=t;");
    assert!(variables.branches.is_empty());
}

#[tokio::test]
async fn named_loaders_handle_empty_groups_without_probing_neighbors_or_unrelated_apis() {
    for teacher in [false, true] {
        let college = College::start(teacher).await;
        Mock::given(path("/assets/main.js"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"
                '/v3/ws/personal/info'; '/v3/ws/unrelated/stats';
                this.loadMy=()=>{e.Get({uri:"/v3/ws/personal/schedule",params:{}})};
                this.loadTeacher=()=>{e.Get({uri:"/v3/ws/personal/schedule",params:{}})};
                this.loadGroup=()=>{e.LectureHallManager.load(null,()=>{e.Get({uri:"/v3/ws/groups/schedule",params:{}})})};
            "#))
            .with_priority(1).mount(&college.server).await;
        Mock::given(path("/v3/ws/groups/schedule"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .with_priority(1)
            .mount(&college.server)
            .await;
        let client = college.login().await;
        let scope = if teacher {
            "teacher:TEACHER"
        } else {
            "personal"
        };
        assert!(
            !client
                .schedule(269, week(), scope)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            client
                .schedule(270, week(), "group:270")
                .await
                .unwrap()
                .is_empty()
        );
        let requests = college.server.received_requests().await.unwrap();
        assert!(
            !requests
                .iter()
                .any(|r| r.url.path() == "/v3/ws/unrelated/stats")
        );
        let groups: Vec<_> = requests
            .iter()
            .filter(|r| r.url.path() == "/v3/ws/groups/schedule")
            .flat_map(|r| {
                r.url
                    .query_pairs()
                    .filter(|(k, _)| k == "Group")
                    .map(|(_, v)| v.into_owned())
            })
            .collect();
        assert!(!groups.iter().any(|g| g == "268"));
        assert_eq!(groups.iter().filter(|g| *g == "270").count(), 1);
    }
}

#[tokio::test]
async fn account_branch_takes_precedence_over_javascript_candidates() {
    let college = College::start(false).await;
    Mock::given(path("/v3/ws/personal/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "IsStudent":true,"Group":"269","Branch":"account-branch"
        })))
        .with_priority(1)
        .mount(&college.server)
        .await;
    let client = college.login().await;
    client.schedule(269, week(), "personal").await.unwrap();
    assert!(!client.references().await.halls.is_empty());
    let requests = college.server.received_requests().await.unwrap();
    assert!(
        requests
            .iter()
            .filter(|r| r.url.path().ends_with("/lecture-hall"))
            .all(|r| r
                .url
                .query_pairs()
                .any(|(k, v)| k == "Branch" && v == "account-branch"))
    );
}

#[tokio::test]
async fn stale_schedule_addresses_are_rediscovered_from_new_assets() {
    let college = College::start(false).await;
    let client = college.login().await;
    client.schedule(269, week(), "personal").await.unwrap();
    Mock::given(path("/v3/ws/personal/schedule"))
        .respond_with(ResponseTemplate::new(404))
        .with_priority(1)
        .mount(&college.server)
        .await;
    Mock::given(path("/assets/main.js"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("'/v3/ws/new/schedule'; '/v3/ws/personal/info';"),
        )
        .with_priority(1)
        .mount(&college.server)
        .await;
    Mock::given(path("/v3/ws/new/schedule"))
        .respond_with(ResponseTemplate::new(200).set_body_json(support::schedule(
            "New address",
            "269",
            true,
        )))
        .mount(&college.server)
        .await;
    let days = client.schedule(269, week(), "personal").await.unwrap();
    assert_eq!(days[0].subjects[0].discipline, "New address");
}

#[tokio::test]
async fn stale_file_hash_is_refreshed_before_retrying_the_download() {
    let college = College::start(false).await;
    let client = college.login().await;
    client.refresh(269, week(), false).await.unwrap();
    Mock::given(path("/v3/ws/files/open"))
        .respond_with(ResponseTemplate::new(404))
        .with_priority(1)
        .mount(&college.server)
        .await;
    Mock::given(path("/assets/main.js"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("'/v3/ws/newfiles/open?ID=1'; '/v3/ws/personal/schedule';"),
        )
        .with_priority(1)
        .mount(&college.server)
        .await;
    Mock::given(path("/v3/ws/newfiles/open"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"Link":"/download/task.pdf","Caption":"new.pdf"}),
            ),
        )
        .mount(&college.server)
        .await;
    let (file, caption) = client.download(10).await.unwrap();
    assert_eq!(caption, "new.pdf");
    assert_eq!(tokio::fs::read(&file).await.unwrap(), b"%PDF-test");
    let path = file.to_path_buf();
    drop(file);
    assert!(!path.exists());
}

#[tokio::test]
async fn oversized_responses_are_rejected_instead_of_truncated_and_parsed() {
    let college = College::start(false).await;
    let client = college.login().await;
    client.refresh(269, week(), false).await.unwrap();
    Mock::given(path("/v3/ws/files/id"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b' '; 4 * 1024 * 1024 + 1]))
        .with_priority(1)
        .mount(&college.server)
        .await;
    assert!(
        client
            .document(10)
            .await
            .unwrap_err()
            .to_string()
            .contains("size limit")
    );
}
