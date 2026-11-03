#![allow(dead_code)]
use ktk_schedule::{
    app::App,
    config::Config,
    model::{decode_seroval, seroval_escape},
    workspace::{
        DEFAULT_DIGITAL_GROUP, DEFAULT_DIGITAL_LOGOUT, DEFAULT_DIGITAL_SIGN_IN,
        DEFAULT_DIGITAL_STUDENT, DEFAULT_DIGITAL_TEACHER, Platform, Site, Workspace,
    },
};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
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
        Workspace::login(&self.site(), "user", "password")
            .await
            .unwrap()
    }
    pub fn site(&self) -> Site {
        Site::new(
            self.server.uri().parse().unwrap(),
            "test",
            Arc::new(Semaphore::new(12)),
        )
    }
}

/// Plain JSON as the seroval node tree a TanStack Start server function answers with,
/// strings escaped exactly like the website escapes them.
pub fn seroval(value: &Value) -> Value {
    fn node(value: &Value, next: &mut i64) -> Value {
        match value {
            Value::Null => json!({"t": 2, "s": 0}),
            Value::Bool(b) => json!({"t": 2, "s": if *b { 2 } else { 3 }}),
            Value::Number(n) => json!({"t": 0, "s": n}),
            Value::String(s) => json!({"t": 1, "s": seroval_escape(s)}),
            Value::Array(items) => {
                let id = *next;
                *next += 1;
                let items: Vec<_> = items.iter().map(|item| node(item, next)).collect();
                json!({"t": 9, "i": id, "l": items.len(), "a": items, "o": 0})
            }
            Value::Object(map) => {
                let id = *next;
                *next += 1;
                let (keys, values): (Vec<_>, Vec<_>) = map
                    .iter()
                    .map(|(k, v)| (seroval_escape(k), node(v, next)))
                    .unzip();
                json!({"t": 10, "i": id, "p": {"k": keys, "v": values}, "o": 0})
            }
        }
    }
    node(value, &mut 0)
}

/// Decoded `data` of a server function request (GET `?payload=` or POST body).
pub fn server_fn_data(request: &Request) -> Value {
    let raw: Value = match request.url.query_pairs().find(|(k, _)| k == "payload") {
        Some((_, payload)) => serde_json::from_str(&payload).unwrap(),
        None => request.body_json().unwrap(),
    };
    decode_seroval(&raw).unwrap()["data"].clone()
}

fn session_cookie(request: &Request) -> Option<String> {
    request
        .headers
        .get("cookie")?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|pair| pair.strip_prefix("education_system_auth_session="))
        .map(Into::into)
}

/// A digital platform week: Monday and Tuesday of `week_start`, one lesson each.
pub fn digital_week(week_start: &str, discipline: &str, subgroup: &str, group: &str) -> Value {
    let monday = chrono::NaiveDate::parse_from_str(week_start, "%Y-%m-%d").unwrap();
    let days: Vec<_> = (0..2)
        .map(|offset| {
            let date = (monday + chrono::Days::new(offset)).to_string();
            json!({"date": date, "pages": [{"pairs": [{"pairNumber": 1, "lesson": {
                "journalId": 7, "date": date, "pairNumber": 1, "groupId": group,
                "subgroup": subgroup, "teacherId": "t-1", "auditoriumId": 389,
                "disciplineShortName": discipline, "disciplineFullName": discipline,
                "pairTypeId": 5, "academicHours": 2, "assessment": {"grade": 5, "mark": 0}
            }}]}]})
        })
        .collect();
    json!({"result": {
        "scope": {"weekStart": week_start},
        "maxPairNumber": 6,
        "days": days,
        "references": {
            "teachers": [{"id": "t-1", "surname": "Иванова", "name": "Анна", "patronymic": "Петровна"}],
            "auditoriums": [{"auditoriumId": 389, "code": "18А", "buildingShortCode": "IT"}],
            "pairTypes": [{"id": 5, "name": "Практика", "billingType": "Practice"}]
        },
        "callPresets": [{"id": 55, "beginTime": "08:00", "steps": [
            {"position": 0, "duration": 90, "breakDuration": 10}
        ]}],
        "bellScopes": [{"assignments": [
            {"date": monday.to_string(), "presetId": 55},
            {"date": (monday + chrono::Days::new(1)).to_string(), "presetId": 55}
        ]}]
    }})
}

fn auth_required() -> Value {
    json!({"error": {"appError": {"type": "appError", "code": "authRequired", "message": "Требуется вход"}}})
}

/// ktk.education-system.ru: TanStack Start server functions answering in seroval.
pub struct Digital {
    pub server: MockServer,
    /// Session cookies the site no longer accepts (expired or revoked).
    pub dead: Arc<Mutex<HashSet<String>>>,
    pub sign_ins: Arc<AtomicUsize>,
    /// Passwords exactly as the site decoded them.
    pub passwords: Arc<Mutex<Vec<String>>>,
    pub revoked: Arc<Mutex<Vec<String>>>,
    pub discipline: Arc<Mutex<String>>,
}

impl Digital {
    /// `subgroup` is what the personal timetable lists for split lessons.
    pub async fn start(teacher: bool, subgroup: &str) -> Self {
        let server = MockServer::start().await;
        let this = Self {
            server,
            dead: Arc::default(),
            sign_ins: Arc::default(),
            passwords: Arc::default(),
            revoked: Arc::default(),
            discipline: Arc::new(Mutex::new("МДК \"Веб\" <1>".into())),
        };
        let (count, passwords) = (this.sign_ins.clone(), this.passwords.clone());
        Mock::given(method("POST"))
            .and(path(DEFAULT_DIGITAL_SIGN_IN))
            .respond_with(move |request: &Request| {
                let data = server_fn_data(request);
                let password = data["password"].as_str().unwrap().to_owned();
                passwords.lock().unwrap().push(password.clone());
                if password == "wrong" {
                    let error = json!({"error": {"appError": {"type": "appError", "code": "authRequired", "message": "Неверный логин или пароль."}}});
                    return ResponseTemplate::new(200).set_body_json(seroval(&error));
                }
                let n = count.fetch_add(1, Ordering::SeqCst) + 1;
                let user = if teacher {
                    json!({"hash": "teacher-hash", "group": null, "branch": "b-1", "isStudent": false})
                } else {
                    json!({"hash": "student-hash", "group": "269", "branch": "b-1", "isStudent": true})
                };
                let session = json!({"id": format!("sess-{n}"), "expiresAt": "2030-01-01T00:00:00.000Z", "application": "web", "mode": "ordinary"});
                ResponseTemplate::new(200)
                    .insert_header(
                        "set-cookie",
                        format!("education_system_auth_session=S{n}; Path=/; HttpOnly; SameSite=Lax"),
                    )
                    .set_body_json(seroval(&json!({"result": {"user": user, "session": session}})))
            })
            .mount(&this.server)
            .await;
        let personal = if teacher {
            DEFAULT_DIGITAL_TEACHER
        } else {
            DEFAULT_DIGITAL_STUDENT
        };
        let (dead, discipline, sub) = (
            this.dead.clone(),
            this.discipline.clone(),
            subgroup.to_owned(),
        );
        Mock::given(method("GET"))
            .and(path(personal))
            .respond_with(move |request: &Request| {
                let alive = session_cookie(request)
                    .is_some_and(|cookie| !dead.lock().unwrap().contains(&cookie));
                if !alive {
                    return ResponseTemplate::new(200).set_body_json(seroval(&auth_required()));
                }
                let week = server_fn_data(request)["weekStart"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                let name = discipline.lock().unwrap().clone();
                ResponseTemplate::new(200)
                    .set_body_json(seroval(&digital_week(&week, &name, &sub, "269")))
            })
            .mount(&this.server)
            .await;
        let dead = this.dead.clone();
        Mock::given(method("GET"))
            .and(path(DEFAULT_DIGITAL_GROUP))
            .respond_with(move |request: &Request| {
                if session_cookie(request).is_none_or(|c| dead.lock().unwrap().contains(&c)) {
                    return ResponseTemplate::new(200).set_body_json(seroval(&auth_required()));
                }
                let data = server_fn_data(request);
                let (week, group) = (
                    data["weekStart"].as_str().unwrap().to_owned(),
                    data["groupId"].as_str().unwrap().to_owned(),
                );
                ResponseTemplate::new(200).set_body_json(seroval(&digital_week(
                    &week,
                    &format!("Group {group}"),
                    "right",
                    &group,
                )))
            })
            .mount(&this.server)
            .await;
        let (dead, revoked) = (this.dead.clone(), this.revoked.clone());
        Mock::given(method("POST"))
            .and(path(DEFAULT_DIGITAL_LOGOUT))
            .respond_with(move |request: &Request| {
                let id = server_fn_data(request)["id"].as_str().unwrap().to_owned();
                revoked.lock().unwrap().push(id);
                if let Some(cookie) = session_cookie(request) {
                    dead.lock().unwrap().insert(cookie);
                }
                ResponseTemplate::new(200).set_body_json(seroval(&json!({"result": {"ok": true}})))
            })
            .mount(&this.server)
            .await;
        this
    }

    pub fn site(&self) -> Site {
        let mut site = Site::new(
            self.server.uri().parse().unwrap(),
            "test",
            Arc::new(Semaphore::new(12)),
        );
        // Loopback mocks stand in for ktk.education-system.ru only in tests.
        site.platform = Platform::Digital;
        site
    }

    pub async fn requests_to(&self, path: &str) -> Vec<Request> {
        self.server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|r| r.url.path() == path)
            .collect()
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
    for method in [
        "answercallbackquery",
        "answerinlinequery",
        "deletemessage",
        "setmycommands",
    ] {
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
    app_at(&college.server, telegram, ":memory:").await
}

/// An App talking to the digital platform mock; `database` may be a file to test restarts.
pub async fn digital_app(digital: &Digital, telegram: &MockServer, database: &str) -> App {
    let mut app = app_at(&digital.server, telegram, database).await;
    app.site = digital.site();
    app
}

/// An App with the VK bot enabled against `vk` (owner VK ID 777).
pub async fn vk_app(college: &College, telegram: &MockServer, vk: &MockServer) -> App {
    let mut config = Config::from_values(|key| match key {
        "BOT_TOKEN" => Some("123456:TEST".into()),
        "CREDENTIALS_SECRET" => Some("x".repeat(32)),
        "DATABASE_PATH" => Some(":memory:".into()),
        "VK_GROUP_TOKEN" => Some("vk_test_token".into()),
        "VK_GROUP_ID" => Some("242576636".into()),
        "OWNER_VK_ID" => Some("777".into()),
        _ => None,
    })
    .unwrap();
    // Only test fixtures bypass the production HTTPS requirement for loopback mocks.
    config.base_url = college.server.uri().parse().unwrap();
    config.vk_api_url = format!("{}/", vk.uri()).parse().unwrap();
    let bot = Bot::new("123456:TEST")
        .set_api_url(telegram.uri().parse().unwrap())
        .throttle(Limits::default());
    App::new(config, bot, "test_bot".into()).await.unwrap()
}

async fn app_at(college: &MockServer, telegram: &MockServer, database: &str) -> App {
    let mut config = Config::from_values(|key| match key {
        "BOT_TOKEN" => Some("123456:TEST".into()),
        "CREDENTIALS_SECRET" => Some("x".repeat(32)),
        "DATABASE_PATH" => Some(database.into()),
        _ => None,
    })
    .unwrap();
    // Only test fixtures bypass the production HTTPS requirement for loopback mocks.
    config.base_url = college.uri().parse().unwrap();
    let bot = Bot::new("123456:TEST")
        .set_api_url(telegram.uri().parse().unwrap())
        .throttle(Limits::default());
    App::new(config, bot, "test_bot".into()).await.unwrap()
}
