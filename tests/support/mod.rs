#![allow(dead_code)]
use ktk_schedule::{app::App, config::Config, workspace::Workspace};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use teloxide::{Bot, adaptors::throttle::Limits, requests::RequesterExt};
use tokio::sync::Semaphore;
use wiremock::{
    Mock, MockServer, Request, ResponseTemplate,
    matchers::{method, path, path_regex},
};

pub struct College {
    pub server: MockServer,
    pub changed: Arc<AtomicBool>,
    pub offline: Arc<AtomicBool>,
}

impl College {
    pub async fn start(teacher: bool) -> Self {
        let server = MockServer::start().await;
        let changed = Arc::new(AtomicBool::new(false));
        let offline = Arc::new(AtomicBool::new(false));
        Mock::given(method("POST"))
            .and(path("/sign-in"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("set-cookie", "session=TEST; Path=/")
                    .set_body_json(json!({"Group":269,"Subgroup":"left"})),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("<script src='/assets/main.js'></script>"),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET")).and(path("/assets/main.js"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"'/v3/ws/personal/info'; '/v3/ws/personal/schedule'; '/v3/ws/groups/schedule'; '/v3/ws/ref/lecture-hall'; '/v3/ws/ref/call-preset'; '/v3/ws/ref/pair-type'; '/v3/ws/files/id?ID=1'; '/v3/ws/home/homework/check'; Branch='main';"#)).mount(&server).await;
        Mock::given(method("GET")).and(path("/v3/ws/personal/info"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"IsStudent":!teacher,"Hash":if teacher {"TEACHER"} else {"STUDENT"},"Group":269}))).mount(&server).await;
        let (change, fail) = (changed.clone(), offline.clone());
        Mock::given(method("GET"))
            .and(path("/v3/ws/personal/schedule"))
            .respond_with(move |request: &Request| {
                if fail.load(Ordering::SeqCst) {
                    return ResponseTemplate::new(503);
                }
                if teacher
                    && !request
                        .url
                        .query_pairs()
                        .any(|(k, v)| k == "Teacher" && v == "TEACHER")
                {
                    return ResponseTemplate::new(403);
                }
                ResponseTemplate::new(200).set_body_json(schedule(
                    if change.load(Ordering::SeqCst) {
                        "Updated"
                    } else {
                        "Personal"
                    },
                    "269",
                    !teacher,
                ))
            })
            .mount(&server)
            .await;
        let fail = offline.clone();
        Mock::given(method("GET"))
            .and(path("/v3/ws/groups/schedule"))
            .respond_with(move |request: &Request| {
                if fail.load(Ordering::SeqCst) {
                    return ResponseTemplate::new(503);
                }
                if request
                    .url
                    .query_pairs()
                    .any(|(k, v)| k == "Teacher" && !v.is_empty())
                {
                    return ResponseTemplate::new(403);
                }
                let group = request
                    .url
                    .query_pairs()
                    .find(|(k, _)| k == "Group")
                    .map(|(_, v)| v.to_string())
                    .unwrap_or_default();
                ResponseTemplate::new(200).set_body_json(schedule(
                    &format!("Group {group}"),
                    &group,
                    false,
                ))
            })
            .mount(&server)
            .await;
        Mock::given(path("/v3/ws/ref/lecture-hall"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"LectureHalls":{"main":[{"ID":1,"Number":"301"}]}})),
            )
            .mount(&server)
            .await;
        Mock::given(path("/v3/ws/ref/call-preset")).respond_with(ResponseTemplate::new(200).set_body_json(json!([{"ID":1,"Begin":"1970-01-01T08:00:00Z","CallSet":[{"PairNumber":1,"Duration":90,"Break":10}]}]))).mount(&server).await;
        Mock::given(path("/v3/ws/ref/pair-type"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!([{"ID":1,"Name":"Лекция","BillingType":"Theoretical"}])),
            )
            .mount(&server)
            .await;
        Mock::given(path("/v3/ws/absence/mark"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!([{ "Digit":4,"Caption":"Болезнь"}])),
            )
            .mount(&server)
            .await;
        Mock::given(path("/v3/ws/files/id"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"ID":10,"Caption":"task.pdf","Icon":"pdf"})),
            )
            .mount(&server)
            .await;
        Mock::given(path("/v3/ws/files/open"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"Link":"/download/task.pdf","Caption":"task.pdf"})),
            )
            .mount(&server)
            .await;
        Mock::given(path("/download/task.pdf"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"%PDF-test".to_vec()))
            .mount(&server)
            .await;
        Mock::given(path("/v3/ws/home/homework/check"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"File":{"ID":10}})))
            .mount(&server)
            .await;
        Self {
            server,
            changed,
            offline,
        }
    }
    pub async fn login(&self) -> Workspace {
        Workspace::login(
            self.server.uri().parse().unwrap(),
            "test",
            "user",
            "password",
            Arc::new(Semaphore::new(12)),
        )
        .await
        .unwrap()
    }
}

pub fn schedule(name: &str, group: &str, grades: bool) -> Value {
    let mut subject = json!({"Pair":1,"Discipline":name,"LectureHall":1,"Group":group,"Subgroup":"middle","ExtraData":{"LectureType":1}});
    if grades {
        subject["Appraisal"] = json!(5);
    }
    json!([{"Date":"2026-09-01T00:00:00Z","CallPreset":1,"Subjects":[subject]}])
}

pub async fn telegram_server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(path_regex("(?i)/bot123456:TEST/sendmessage")).respond_with(|request:&Request| {
        let data:Value=request.body_json().unwrap();
        ResponseTemplate::new(200).set_body_json(json!({"ok":true,"result":{"message_id":100,"date":1788202800,"chat":{"id":data["chat_id"],"type":"private"},"text":data["text"]}}))
    }).mount(&server).await;
    Mock::given(path_regex("(?i)/bot123456:TEST/editmessagetext")).respond_with(|request:&Request| {
        let data:Value=request.body_json().unwrap();
        ResponseTemplate::new(200).set_body_json(json!({"ok":true,"result":{"message_id":data["message_id"],"date":1788202800,"chat":{"id":data["chat_id"],"type":"private"},"text":data["text"]}}))
    }).mount(&server).await;
    for method in ["answercallbackquery", "deletemessage"] {
        Mock::given(path_regex(format!("(?i)/bot123456:TEST/{method}")))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"ok":true,"result":true})),
            )
            .mount(&server)
            .await;
    }
    server
}

pub async fn app(college: &College, telegram: &MockServer) -> App {
    let config = Config::from_values(|key| match key {
        "BOT_TOKEN" => Some("123456:TEST".into()),
        "CREDENTIALS_SECRET" => Some("x".repeat(32)),
        "DATABASE_PATH" => Some(":memory:".into()),
        "KTK_BASE_URL" => Some(college.server.uri()),
        _ => None,
    })
    .unwrap();
    let bot = Bot::new("123456:TEST")
        .set_api_url(telegram.uri().parse().unwrap())
        .throttle(Limits::default());
    App::new(config, bot, "test_bot".into()).await.unwrap()
}
