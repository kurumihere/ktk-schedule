use crate::model::*;
use anyhow::{Context, Result, bail, ensure};
use futures::{StreamExt, stream};
use regex::Regex;
use reqwest::{
    Client as HttpClient, Method, RequestBuilder, Response, StatusCode, Url,
    cookie::{CookieStore, Jar},
    header::{HeaderMap, HeaderValue},
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        Arc, LazyLock, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::AsyncWriteExt,
    sync::{Mutex, RwLock, Semaphore, SemaphorePermit},
};

const JSON_LIMIT: usize = 4 * 1024 * 1024;
pub const FILE_LIMIT: usize = 50 * 1024 * 1024;
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/156.0.0.0 Safari/537.36";
const DIGITAL_HOST: &str = "ktk.education-system.ru";
/// Discovery caps: pages may reference many scripts, the bot fetches a bounded number.
const MAX_SCRIPTS: usize = 64;
const MAX_CANDIDATES: usize = 128;

#[derive(Debug)]
pub struct ApiError(pub Option<StatusCode>);
impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Some(status) => write!(f, "workspace returned {status}"),
            None => f.write_str("workspace returned an unexpected response"),
        }
    }
}
impl std::error::Error for ApiError {}
pub fn unauthorized(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<ApiError>()
        .is_some_and(|e| matches!(e.0, Some(StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)))
}
fn rediscover(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<ApiError>()
        .is_some_and(|e| matches!(e.0, None | Some(StatusCode::NOT_FOUND | StatusCode::GONE)))
}

/// The site refused the login and password, as opposed to being unreachable.
#[derive(Debug)]
pub struct CredentialsRejected(pub String);
impl std::fmt::Display for CredentialsRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "credentials rejected: {}", self.0)
    }
}
impl std::error::Error for CredentialsRejected {}

/// Server functions report failures in-band (HTTP 200 with `{ error: { appError: { code } } }`).
/// An expired or revoked session must look like a 401, or the bot keeps serving stale data.
fn server_error(value: &Value) -> Option<anyhow::Error> {
    let error = value.get("error").filter(|e| !e.is_null())?;
    let details = error.get("appError").unwrap_or(error);
    let field = |key: &str| {
        details
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase()
    };
    let (code, kind) = (field("code"), field("type"));
    let auth = ["auth", "session", "unauthor", "forbidden", "login"]
        .iter()
        .any(|word| code.contains(word) || kind.contains(word));
    Some(ApiError(auth.then_some(StatusCode::UNAUTHORIZED)).into())
}

/// Which website the bot talks to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    /// workspace.ktk-45.ru: REST endpoints found in the JavaScript bundles.
    Legacy,
    /// ktk.education-system.ru: TanStack Start server functions speaking seroval.
    Digital,
}

impl Platform {
    pub fn detect(base: &Url) -> Self {
        if base.host_str() == Some(DIGITAL_HOST) {
            Self::Digital
        } else {
            Self::Legacy
        }
    }
}

/// What every session of one bot shares: the site, the request budget and what discovery
/// learned about the site's current deployment.
#[derive(Clone)]
pub struct Site {
    pub base: Url,
    pub platform: Platform,
    pub device: String,
    pub permits: Arc<Semaphore>,
    discovery: Arc<Mutex<Discovery>>,
}

impl Site {
    pub fn new(base: Url, device: impl Into<String>, permits: Arc<Semaphore>) -> Self {
        Self {
            platform: Platform::detect(&base),
            base,
            device: device.into(),
            permits,
            discovery: Arc::default(),
        }
    }
}

/// Discovery results are the same for every account, so one crawl serves all sessions.
/// `generation` lets a session that saw a stale address force exactly one fresh crawl.
#[derive(Default)]
struct Discovery {
    generation: u64,
    candidates: Option<Candidates>,
}

#[derive(Clone, Default)]
struct Endpoints {
    personal: String,
    group: String,
    halls: String,
    calls: String,
    types: String,
    absence: String,
    file_hash: String,
    homework_hash: String,
    branch: String,
    sign_in: String,
    logout: String,
    student_homework: String,
    student_webinar: String,
    group_homework: String,
    group_webinar: String,
}

/// Digital platform server functions the bot calls.
#[derive(Clone, Copy, Debug)]
enum ServerFn {
    SignIn,
    Student,
    Teacher,
    Group,
    Logout,
    /// A lesson's task and files: `{weekStart, journalId}` (group: `+ groupId`).
    StudentHomework,
    StudentWebinar,
    GroupHomework,
    GroupWebinar,
}

impl ServerFn {
    fn method(self) -> Method {
        match self {
            Self::SignIn | Self::Logout => Method::POST,
            _ => Method::GET,
        }
    }

    fn default_path(self) -> &'static str {
        match self {
            Self::SignIn => DEFAULT_DIGITAL_SIGN_IN,
            Self::Student => DEFAULT_DIGITAL_STUDENT,
            Self::Teacher => DEFAULT_DIGITAL_TEACHER,
            Self::Group => DEFAULT_DIGITAL_GROUP,
            Self::Logout => DEFAULT_DIGITAL_LOGOUT,
            Self::StudentHomework => DEFAULT_DIGITAL_STUDENT_HOMEWORK,
            Self::StudentWebinar => DEFAULT_DIGITAL_STUDENT_WEBINAR,
            Self::GroupHomework => DEFAULT_DIGITAL_GROUP_HOMEWORK,
            Self::GroupWebinar => DEFAULT_DIGITAL_GROUP_WEBINAR,
        }
    }

    fn discovered(self, candidates: &Candidates) -> Option<&String> {
        match self {
            Self::SignIn => candidates.digital_sign_in.as_ref(),
            Self::Student => candidates.digital_student_schedule.as_ref(),
            Self::Teacher => candidates.digital_teacher_schedule.as_ref(),
            Self::Group => candidates.digital_group_schedule.as_ref(),
            Self::Logout => candidates.digital_logout.as_ref(),
            // Not told apart by discovery yet: the known ids are used.
            Self::StudentHomework
            | Self::StudentWebinar
            | Self::GroupHomework
            | Self::GroupWebinar => None,
        }
    }

    fn slot(self, endpoints: &mut Endpoints) -> &mut String {
        match self {
            Self::SignIn => &mut endpoints.sign_in,
            Self::Student | Self::Teacher => &mut endpoints.personal,
            Self::Group => &mut endpoints.group,
            Self::Logout => &mut endpoints.logout,
            Self::StudentHomework => &mut endpoints.student_homework,
            Self::StudentWebinar => &mut endpoints.student_webinar,
            Self::GroupHomework => &mut endpoints.group_homework,
            Self::GroupWebinar => &mut endpoints.group_webinar,
        }
    }
}

#[derive(Clone, Default)]
pub struct Candidates {
    pub info: Vec<String>,
    pub schedules: Vec<String>,
    pub personal_schedules: Vec<String>,
    pub group_schedules: Vec<String>,
    pub teacher_schedules: Vec<String>,
    pub halls: Vec<String>,
    pub calls: Vec<String>,
    pub types: Vec<String>,
    pub files: Vec<String>,
    pub homework: Vec<String>,
    pub branches: Vec<String>,
    pub digital_sign_in: Option<String>,
    pub digital_student_schedule: Option<String>,
    pub digital_teacher_schedule: Option<String>,
    pub digital_group_schedule: Option<String>,
    pub digital_logout: Option<String>,
}

pub const DEFAULT_DIGITAL_SIGN_IN: &str =
    "/_serverFn/9cd5b48fd0588913cb64c83d8ae2b42d5f3e97ed5dd47dc10f211cdc2322c1f6";
pub const DEFAULT_DIGITAL_STUDENT: &str =
    "/_serverFn/85d2195b8f64b023a2c16160b65ef47139dea3a839096c4b75b3888cda49f16c";
pub const DEFAULT_DIGITAL_TEACHER: &str =
    "/_serverFn/9fd2616a2ecddd56f79974f2111a13f2c9d89265cb6832c8c9e7ae2939e56f4a";
pub const DEFAULT_DIGITAL_GROUP: &str =
    "/_serverFn/d418f54f8d5ba132bd168b1792cc8f0efbff634c7861f6aa66890cb9f7103c05";
pub const DEFAULT_DIGITAL_LOGOUT: &str =
    "/_serverFn/2e1ca1208f926608597adee62b6f7a786ebfd7e01b208963ecb9bee6e800f9f5";
/// `ownStudentLessonHomework`, `ownStudentLessonWebinar`, `groupLessonHomework` and
/// `groupLessonWebinar` of the timetable page (bundle of 2026-11-03).
pub const DEFAULT_DIGITAL_STUDENT_HOMEWORK: &str =
    "/_serverFn/6856a131ea327da0ae2769d0e8b9950bce030194dda15acd331dd5ad2c273f38";
pub const DEFAULT_DIGITAL_STUDENT_WEBINAR: &str =
    "/_serverFn/b218948fba15961755e1cf6c0742e352d8709719e99ac7bd3e06315787e13650";
pub const DEFAULT_DIGITAL_GROUP_HOMEWORK: &str =
    "/_serverFn/c45239ec7e4dc5b1d502259920ddafa06eec8ce3391ba127bd60b4e86e07c60b";
pub const DEFAULT_DIGITAL_GROUP_WEBINAR: &str =
    "/_serverFn/b45e772e2c78dafc8e71009e176b018dc30498e626381ce78c54098424cb9335";
/// At most this many task/webinar requests per digital week.
const MAX_LESSON_EXTRAS: usize = 40;

static SERVER_FN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"/_serverFn/([0-9a-fA-F]{32,64})"#).unwrap());

static API: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/v[0-9]+/[A-Za-z0-9_-]+/[A-Za-z0-9_-]+/[A-Za-z0-9_-]+").unwrap());
static SCRIPTS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"["']([^"']+\.js(?:\?[^"']*)?)["']"#).unwrap());
static SCRIPT_SRC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)<script[^>]+src=["']([^"']+)["']"#).unwrap());
static FILE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"/v[0-9]+/[A-Za-z0-9_-]+/([A-Za-z0-9_-]+)/(?:user-file|id|open|can-view)(?:[?"'/\s]|$)"#,
    )
    .unwrap()
});
static HOMEWORK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"/v[0-9]+/[A-Za-z0-9_-]+/([A-Za-z0-9_-]+)/homework/check").unwrap()
});
// The page names these loaders explicitly; use that meaning before probing APIs.
static SCHEDULE_LOADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"this\.load(My|Group|Teacher)\s*=\s*\(\)\s*=>\s*\{[^;]{0,240}?uri\s*:\s*["'](/v[0-9]+/[A-Za-z0-9_-]+/[A-Za-z0-9_-]+/[A-Za-z0-9_-]+)["']"#).unwrap()
});
static BRANCH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?:Branch%3[Dd]|[?&]Branch=|Branch["']?\s*[:=]\s*["'])([A-Za-z0-9_-]+)"#).unwrap()
});

fn unique(list: &mut Vec<String>, value: &str) {
    if !value.is_empty() && list.len() < MAX_CANDIDATES && !list.iter().any(|s| s == value) {
        list.push(value.into());
    }
}

impl Candidates {
    pub fn add(&mut self, text: &str) {
        for c in SCHEDULE_LOADER.captures_iter(text) {
            let dest = match &c[1] {
                "My" => &mut self.personal_schedules,
                "Group" => &mut self.group_schedules,
                _ => &mut self.teacher_schedules,
            };
            unique(dest, &c[2]);
        }
        for p in API.find_iter(text).map(|m| m.as_str()) {
            let dest = match p.rsplit('/').next().unwrap_or_default() {
                "info" => &mut self.info,
                "lecture-hall" => &mut self.halls,
                "call-preset" => &mut self.calls,
                "pair-type" => &mut self.types,
                "id" | "open" | "can-view" | "user-file" | "homework" => continue,
                _ => &mut self.schedules,
            };
            unique(dest, p);
        }
        for (pattern, dest) in [
            (&*FILE, &mut self.files),
            (&*HOMEWORK, &mut self.homework),
            (&*BRANCH, &mut self.branches),
        ] {
            for captures in pattern.captures_iter(text) {
                unique(dest, &captures[1]);
            }
        }
        for m in SERVER_FN.find_iter(text) {
            let path = m.as_str();
            let idx = m.start();
            let prev_fn = text[..idx]
                .rfind("/_serverFn/")
                .map(|i| i + "/_serverFn/".len())
                .unwrap_or(0);
            let next_fn = text[idx + path.len()..]
                .find("/_serverFn/")
                .map(|i| idx + path.len() + i)
                .unwrap_or(text.len());
            let after_idx = idx + path.len();

            let before_part = &text[prev_fn..idx];
            let start = before_part
                .rfind(';')
                .or_else(|| before_part.rfind('\n'))
                .map(|i| prev_fn + i + 1)
                .unwrap_or(prev_fn);

            let after_part = &text[after_idx..next_fn];
            let end = after_part
                .find(';')
                .or_else(|| after_part.find("createServerFn"))
                .map(|i| after_idx + i)
                .unwrap_or(next_fn);

            let snippet = if start <= end {
                &text[start..end]
            } else {
                &text[idx..after_idx]
            };

            if snippet.contains("collegeId") || snippet.contains("fingerprint") {
                if self.digital_sign_in.is_none() {
                    self.digital_sign_in = Some(path.to_string());
                }
            } else if snippet.contains("groupId") && snippet.contains("weekStart") {
                if self.digital_group_schedule.is_none() {
                    self.digital_group_schedule = Some(path.to_string());
                }
            } else if snippet.contains("weekStart") {
                if snippet.contains("teacher") || snippet.contains("Teacher") {
                    if self.digital_teacher_schedule.is_none() {
                        self.digital_teacher_schedule = Some(path.to_string());
                    }
                } else if self.digital_student_schedule.is_none() {
                    self.digital_student_schedule = Some(path.to_string());
                }
            } else if (snippet.contains("revoke")
                || snippet.contains("logout")
                || snippet.contains("signOut")
                || snippet.contains("sign-out"))
                && self.digital_logout.is_none()
            {
                self.digital_logout = Some(path.to_string());
            }
        }
    }
}

pub fn script_urls(text: &str, base: &Url) -> Vec<Url> {
    let mut seen = HashSet::new();
    SCRIPT_SRC
        .captures_iter(text)
        .chain(SCRIPTS.captures_iter(text))
        .filter_map(|c| base.join(&c[1]).ok())
        .filter(|u| u.origin() == base.origin() && seen.insert(u.clone()))
        .take(MAX_CANDIDATES)
        .collect()
}

/// Browser-like client confined to the site: same-origin redirects only, never with userinfo.
fn http_client(base: &Url, jar: Arc<Jar>) -> Result<HttpClient> {
    let origin = base.origin();
    let mut headers = HeaderMap::new();
    for (name, value) in [
        ("sec-fetch-site", "same-origin"),
        ("sec-fetch-mode", "cors"),
        ("sec-fetch-dest", "empty"),
    ] {
        headers.insert(name, HeaderValue::from_static(value));
    }
    Ok(HttpClient::builder()
        .cookie_provider(jar)
        .default_headers(headers)
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= 5 {
                attempt.error("too many redirects")
            } else if attempt.url().origin() != origin {
                attempt.error("cross-origin redirect is forbidden")
            } else if !attempt.url().username().is_empty() || attempt.url().password().is_some() {
                attempt.error("redirect credentials are forbidden")
            } else {
                attempt.follow()
            }
        }))
        .build()?)
}

/// Stable per account and device, so the site recognizes the bot as one device.
fn device_fingerprint(login: &str, device: &str) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::new()
        .chain_update(login.as_bytes())
        .chain_update(device.as_bytes())
        .finalize();
    hash[..16].iter().map(|b| format!("{b:02x}")).collect()
}

/// A digital task file: downloads go to the lesson's file route of the same timetable.
#[derive(Clone, Debug)]
struct DigitalFile {
    caption: String,
    extension: String,
    journal: i64,
    week_start: String,
    /// `None` for the own timetable (`students/self`).
    group: Option<String>,
}

/// Digital file names are remembered per session up to this many.
const MAX_DIGITAL_FILES: usize = 4096;

pub struct Workspace {
    http: HttpClient,
    jar: Arc<Jar>,
    base: Url,
    platform: Platform,
    permits: Arc<Semaphore>,
    discovery: Arc<Mutex<Discovery>>,
    discovery_seen: AtomicU64,
    endpoints: RwLock<Endpoints>,
    refresh_lock: Mutex<()>,
    /// Halls, presets and pair types accumulated from every digital week this session read.
    digital_refs: RwLock<Arc<ReferenceData>>,
    /// Task files seen in digital weeks: their names and where to download them.
    digital_files: std::sync::Mutex<HashMap<i64, DigitalFile>>,
    pub subgroup: String,
    pub group: i64,
    pub teacher: String,
    branch: String,
    pub session_id: String,
}

impl Workspace {
    fn new(site: &Site, jar: Arc<Jar>) -> Result<Self> {
        Ok(Self {
            http: http_client(&site.base, jar.clone())?,
            jar,
            base: site.base.clone(),
            platform: site.platform,
            permits: site.permits.clone(),
            discovery: site.discovery.clone(),
            discovery_seen: AtomicU64::new(0),
            endpoints: RwLock::default(),
            refresh_lock: Mutex::new(()),
            digital_refs: RwLock::default(),
            digital_files: std::sync::Mutex::default(),
            subgroup: String::new(),
            group: 0,
            teacher: String::new(),
            branch: String::new(),
            session_id: String::new(),
        })
    }

    pub async fn login(site: &Site, login: &str, password: &str) -> Result<Self> {
        let mut this = Self::new(site, Arc::new(Jar::default()))?;
        match site.platform {
            Platform::Digital => this.digital_login(&site.device, login, password).await?,
            Platform::Legacy => this.legacy_login(&site.device, login, password).await?,
        }
        Ok(this)
    }

    async fn digital_login(&mut self, device: &str, login: &str, password: &str) -> Result<()> {
        let (login, password) = (login.trim(), password.trim());
        let fingerprint = device_fingerprint(login, device);
        let result = self
            .call(
                ServerFn::SignIn,
                &[
                    ("collegeId", login),
                    ("password", password),
                    ("fingerprint", &fingerprint),
                ],
            )
            .await?;
        let user = &result["user"];
        if user.is_null() {
            let message = result["error"]["appError"]["message"]
                .as_str()
                .or_else(|| result["error"]["message"].as_str())
                .unwrap_or("unauthorized");
            return Err(CredentialsRejected(message.into()).into());
        }
        self.session_id = result["session"]["id"]
            .as_str()
            .or_else(|| user["session"]["id"].as_str())
            .unwrap_or_default()
            .into();
        if user["isStudent"].as_bool().unwrap_or(true) {
            self.group = positive_id(&user["group"]).unwrap_or(0);
            // The account does not name a subgroup; App infers it from the timetable.
            self.subgroup = find_subgroup(user).unwrap_or_default().into();
        } else {
            self.teacher = user["hash"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or("teacher")
                .into();
            self.group = 0;
        }
        self.branch = user["branch"].as_str().unwrap_or_default().into();
        self.endpoints.get_mut().branch = self.branch.clone();
        Ok(())
    }

    async fn legacy_login(&mut self, device: &str, login: &str, password: &str) -> Result<()> {
        let url = self.base.join("/sign-in")?;
        let credentials = json!({"Login": login, "Password": password, "Device": device});
        let (response, permit) = self
            .send(
                || {
                    self.http
                        .post(url.clone())
                        .header("Origin", self.base.origin().ascii_serialization())
                        .header("Referer", self.base.as_str())
                        .json(&credentials)
                },
                false,
            )
            .await?;
        if matches!(
            response.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ) {
            return Err(CredentialsRejected(response.status().to_string()).into());
        }
        ensure_status(&response)?;
        let body = read_body(response, JSON_LIMIT).await?;
        drop(permit);
        if let Ok(value) = serde_json::from_slice::<Value>(&body) {
            self.subgroup = find_subgroup(&value).unwrap_or_default().into();
            self.group = find_group(&value).unwrap_or(0);
        }
        let candidates = self.discover(false).await?;
        let mut info_paths = candidates.info.clone();
        for path in candidates.schedules.iter().chain(candidates.calls.iter()) {
            unique(&mut info_paths, &sibling(path, "info"));
        }
        let mut info = None;
        for path in info_paths {
            if let Ok(value) = self.get_json::<Value>(&path, &[]).await
                && value["IsStudent"].is_boolean()
            {
                info = Some(value);
                break;
            }
        }
        let info = info.context("could not determine workspace account type")?;
        if let Some(group) = find_group(&info) {
            self.group = group;
        }
        if let Some(subgroup) = find_subgroup(&info) {
            self.subgroup = subgroup.into();
        }
        self.branch = info["Branch"].as_str().unwrap_or_default().into();
        if info["IsStudent"] == false {
            self.teacher = info["Hash"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or("teacher")
                .into();
        }
        Ok(())
    }

    /// Rebuild a signed-in session from its sealed record; nothing is sent to the site.
    pub fn from_session(site: &Site, record: &SessionRecord) -> Result<Self> {
        let jar = Arc::new(Jar::default());
        for pair in record
            .cookie
            .split(';')
            .map(str::trim)
            .filter(|p| !p.is_empty())
        {
            jar.add_cookie_str(pair, &site.base);
        }
        ensure!(
            jar.cookies(&site.base).is_some(),
            "saved session has no usable cookie"
        );
        let mut this = Self::new(site, jar)?;
        // Digital function ids are worth keeping; legacy endpoints are probed per session
        // (halls, presets, file hashes), so a restored legacy session discovers them again.
        if site.platform == Platform::Digital {
            *this.endpoints.get_mut() = Endpoints {
                personal: record.personal_endpoint.clone(),
                group: record.group_endpoint.clone(),
                branch: record.branch.clone(),
                ..Default::default()
            };
        }
        this.subgroup = record.subgroup.clone();
        this.group = record.group;
        this.teacher = record.teacher.clone();
        this.branch = record.branch.clone();
        this.session_id = record.session_id.clone();
        Ok(this)
    }

    pub async fn session_record(&self) -> SessionRecord {
        let ep = self.endpoints.read().await;
        SessionRecord {
            cookie: self.cookie_header(),
            session_id: self.session_id.clone(),
            group: self.group,
            subgroup: self.subgroup.clone(),
            teacher: self.teacher.clone(),
            branch: self.branch.clone(),
            personal_endpoint: ep.personal.clone(),
            group_endpoint: ep.group.clone(),
        }
    }

    /// Every cookie the site set for this session, as a `Cookie` header value.
    pub fn cookie_header(&self) -> String {
        self.jar
            .cookies(&self.base)
            .and_then(|header| header.to_str().ok().map(Into::into))
            .unwrap_or_default()
    }

    pub fn platform(&self) -> Platform {
        self.platform
    }

    /// End the session on the site, so it does not linger in the account's session list.
    pub async fn revoke_session(&self) -> Result<()> {
        if self.platform != Platform::Digital || self.session_id.is_empty() {
            return Ok(());
        }
        let result = self
            .call(ServerFn::Logout, &[("id", &self.session_id)])
            .await?;
        server_error(&result).map_or(Ok(()), Err)
    }

    /// Crawl the page and its scripts once per deployment; `force` skips a cached crawl unless
    /// another session already refreshed it since this session last looked.
    pub async fn discover(&self, force: bool) -> Result<Candidates> {
        let mut shared = self.discovery.lock().await;
        let seen = self.discovery_seen.load(Ordering::Relaxed);
        if let Some(candidates) = &shared.candidates
            && (!force || shared.generation > seen)
        {
            self.discovery_seen
                .store(shared.generation, Ordering::Relaxed);
            return Ok(candidates.clone());
        }
        let candidates = self.crawl().await?;
        shared.generation += 1;
        shared.candidates = Some(candidates.clone());
        self.discovery_seen
            .store(shared.generation, Ordering::Relaxed);
        Ok(candidates)
    }

    async fn crawl(&self) -> Result<Candidates> {
        let root = self.base.join("/")?;
        let html = self.get_text(root.clone()).await?;
        let mut candidates = Candidates::default();
        candidates.add(&html);
        let mut queue: VecDeque<_> = script_urls(&html, &root).into();
        let mut seen = HashSet::new();
        while !queue.is_empty() && seen.len() < MAX_SCRIPTS {
            let mut batch = Vec::new();
            while batch.len() < 4 && seen.len() < MAX_SCRIPTS {
                let Some(url) = queue.pop_front() else { break };
                if url.origin() == self.base.origin() && seen.insert(url.clone()) {
                    batch.push(url);
                }
            }
            let results = stream::iter(batch.into_iter().map(|url| async move {
                let result = self.get_text(url.clone()).await;
                (url, result)
            }))
            .boxed()
            .buffered(4)
            .collect::<Vec<_>>()
            .await;
            for (url, result) in results {
                if let Ok(text) = result {
                    candidates.add(&text);
                    for next in script_urls(&text, &url) {
                        if !seen.contains(&next) && queue.len() < 256 {
                            queue.push_back(next);
                        }
                    }
                }
            }
        }
        Ok(candidates)
    }

    /// Send under a request permit, retrying idempotent requests on 5xx, connection errors
    /// and timeouts. The permit is returned so the caller reads the body under it.
    async fn send(
        &self,
        request: impl Fn() -> RequestBuilder,
        idempotent: bool,
    ) -> Result<(Response, SemaphorePermit<'_>)> {
        let attempts = if idempotent { 3 } else { 1 };
        let mut attempt = 0;
        loop {
            let permit = self.permits.acquire().await?;
            attempt += 1;
            let retry = attempt < attempts;
            match request().send().await {
                Ok(response) if retry && response.status().is_server_error() => (),
                Ok(response) => return Ok((response, permit)),
                Err(error) if retry && (error.is_connect() || error.is_timeout()) => (),
                Err(error) => return Err(error.without_url().into()),
            }
            drop(permit);
            tokio::time::sleep(Duration::from_millis(100 << (attempt - 1))).await;
        }
    }

    fn endpoint_url(&self, path: &str) -> Result<Url> {
        ensure!(!path.is_empty(), "workspace endpoint unavailable");
        let url = self.base.join(path)?;
        ensure!(
            url.origin() == self.base.origin(),
            "API endpoint has a different origin"
        );
        Ok(url)
    }

    async fn request(&self, url: Url, limit: usize) -> Result<Vec<u8>> {
        let (response, _permit) = self
            .send(
                || {
                    self.http
                        .get(url.clone())
                        .header("Referer", self.base.as_str())
                        .header("Accept", "application/json,text/html,*/*")
                        .header("X-Requested-With", "XMLHttpRequest")
                },
                true,
            )
            .await?;
        ensure_status(&response)?;
        read_body(response, limit).await
    }

    async fn get_text(&self, url: Url) -> Result<String> {
        Ok(String::from_utf8(self.request(url, JSON_LIMIT).await?)?)
    }

    async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T> {
        let mut url = self.endpoint_url(path)?;
        url.query_pairs_mut()
            .extend_pairs(query.iter().map(|(k, v)| (*k, v.as_str())));
        let bytes = self.request(url, JSON_LIMIT).await?;
        serde_json::from_slice(&bytes).map_err(|_| ApiError(None).into())
    }

    /// One server function request, framed like the website's own client does it.
    async fn call_once(
        &self,
        function: ServerFn,
        path: &str,
        fields: &[(&str, Field<'_>)],
    ) -> Result<Value> {
        let url = self.endpoint_url(path)?;
        let payload = seroval_fields(fields);
        let referer = match function {
            ServerFn::SignIn => self.base.join("/welcome?redirect=%2F")?,
            _ => self.base.clone(),
        };
        let method = function.method();
        let (response, _permit) = self
            .send(
                || {
                    let request = if method == Method::GET {
                        let mut url = url.clone();
                        url.query_pairs_mut()
                            .append_pair("payload", &payload.to_string());
                        self.http.get(url)
                    } else {
                        self.http.post(url.clone()).json(&payload)
                    };
                    request
                        .header("Origin", self.base.origin().ascii_serialization())
                        .header("Referer", referer.as_str())
                        .header("x-tsr-serverfn", "true")
                        .header("x-application-storage-mode", "ordinary")
                        .header(
                            "Accept",
                            "application/x-tss-framed, application/x-ndjson, application/json",
                        )
                },
                method == Method::GET,
            )
            .await?;
        ensure_status(&response)?;
        let body = read_body(response, JSON_LIMIT).await?;
        let raw: Value = serde_json::from_slice(&body).map_err(|_| ApiError(None))?;
        decode_seroval(&raw)
    }

    /// Call a server function; after a redeploy its id changes, so look it up once again.
    async fn call(&self, function: ServerFn, fields: &[(&str, &str)]) -> Result<Value> {
        let fields: Vec<_> = fields.iter().map(|(k, v)| (*k, Field::Text(v))).collect();
        self.call_fields(function, &fields).await
    }

    async fn call_fields(&self, function: ServerFn, fields: &[(&str, Field<'_>)]) -> Result<Value> {
        let path = {
            let mut endpoints = self.endpoints.write().await;
            let slot = function.slot(&mut endpoints);
            if slot.is_empty() {
                *slot = function.default_path().into();
            }
            slot.clone()
        };
        let error = match self.call_once(function, &path, fields).await {
            Err(error) if rediscover(&error) => error,
            result => return result,
        };
        let Ok(candidates) = self.discover(true).await else {
            return Err(error);
        };
        let Some(found) = function
            .discovered(&candidates)
            .filter(|found| **found != path)
            .cloned()
        else {
            return Err(error);
        };
        tracing::info!(function = ?function, "server function moved; using rediscovered address");
        *function.slot(&mut *self.endpoints.write().await) = found.clone();
        self.call_once(function, &found, fields).await
    }

    async fn raw_schedule(
        &self,
        path: &str,
        group: i64,
        teacher: &str,
        week: Week,
    ) -> Result<(Vec<ScheduleDay>, bool)> {
        let mut query = vec![
            (
                "Group",
                if teacher.is_empty() {
                    group.to_string()
                } else {
                    String::new()
                },
            ),
            ("Teacher", teacher.into()),
            ("Week", week.millis.to_string()),
        ];
        if !teacher.is_empty() {
            query.push(("Year", week.academic_year().to_string()));
        }
        let value: Value = self.get_json(path, &query).await?;
        let has_grades = has_grade_fields(&value);
        let days = parse_schedule(&value).map_err(|_| ApiError(None))?;
        Ok((days, has_grades))
    }

    pub async fn refresh(&self, group: i64, week: Week, force: bool) -> Result<()> {
        let _guard = self.refresh_lock.lock().await;
        if !force && !self.endpoints.read().await.personal.is_empty() {
            return Ok(());
        }
        let candidates = self.discover(force).await?;
        let preferred = if self.teacher.is_empty() {
            &candidates.personal_schedules
        } else {
            &candidates.teacher_schedules
        };
        let paths: Vec<_> = if !preferred.is_empty() && !candidates.group_schedules.is_empty() {
            preferred
                .iter()
                .chain(&candidates.group_schedules)
                .collect()
        } else {
            candidates.schedules.iter().collect()
        };
        let results = stream::iter(paths.into_iter().map(|path| {
            let known_group = candidates.group_schedules.contains(path);
            let known_personal = preferred.contains(path);
            async move {
                let (days, grades) = self
                    .raw_schedule(path, group, &self.teacher, week)
                    .await
                    .ok()?;
                let count = days.iter().map(|d| d.subjects.len()).sum::<usize>();
                let mut group_aware = known_group;
                if self.teacher.is_empty() && !known_group && !known_personal {
                    for other in [group.saturating_sub(1), group.saturating_add(1)] {
                        if other <= 0 {
                            continue;
                        }
                        if let Ok((other_days, _)) = self.raw_schedule(path, other, "", week).await
                            && other_days.iter().any(|d| !d.subjects.is_empty())
                            && lesson_fingerprint(&other_days) != lesson_fingerprint(&days)
                        {
                            group_aware = true;
                            break;
                        }
                    }
                }
                Some((path.clone(), grades, count, group_aware))
            }
        }))
        .boxed()
        .buffered(4)
        .filter_map(|r| async { r })
        .collect::<Vec<_>>()
        .await;
        let personal = results
            .iter()
            .filter(|r| preferred.contains(&r.0))
            .max_by_key(|r| (r.1, r.2))
            .or_else(|| results.iter().filter(|r| !r.3).max_by_key(|r| (r.1, r.2)))
            .or_else(|| results.iter().max_by_key(|r| r.2));
        let personal = personal.context("schedule endpoint not found")?.0.clone();
        let mut group_path = results
            .iter()
            .filter(|r| r.3)
            .max_by_key(|r| r.2)
            .map(|r| r.0.clone())
            .unwrap_or_default();
        if !self.teacher.is_empty() {
            let group_candidates = if candidates.group_schedules.is_empty() {
                &candidates.schedules
            } else {
                &candidates.group_schedules
            };
            let groups = stream::iter(group_candidates.iter().map(|path| {
                let known_group = candidates.group_schedules.contains(path);
                async move {
                    let (days, _) = self.raw_schedule(path, group, "", week).await.ok()?;
                    if known_group {
                        return Some((
                            path.clone(),
                            days.iter().map(|d| d.subjects.len()).sum::<usize>(),
                        ));
                    }
                    for other in [group.saturating_sub(1), group.saturating_add(1)] {
                        if other <= 0 {
                            continue;
                        }
                        if let Ok((other_days, _)) = self.raw_schedule(path, other, "", week).await
                            && other_days.iter().any(|d| !d.subjects.is_empty())
                            && lesson_fingerprint(&other_days) != lesson_fingerprint(&days)
                        {
                            return Some((
                                path.clone(),
                                days.iter().map(|d| d.subjects.len()).sum::<usize>(),
                            ));
                        }
                    }
                    None
                }
            }))
            .boxed()
            .buffered(4)
            .filter_map(|r| async { r })
            .collect::<Vec<_>>()
            .await;
            group_path = groups
                .into_iter()
                .max_by_key(|(_, count)| *count)
                .map(|(path, _)| path)
                .unwrap_or_default();
        }
        let calls = self
            .first_valid::<Vec<Preset>>(&candidates.calls)
            .await
            .unwrap_or_else(|| sibling(&personal, "call-preset"));
        let types = self
            .first_valid::<Vec<PairType>>(&candidates.types)
            .await
            .unwrap_or_default();
        let branch = if self.branch.is_empty() {
            candidates.branches.first().cloned().unwrap_or_default()
        } else {
            self.branch.clone()
        };
        let mut halls = String::new();
        for path in &candidates.halls {
            if let Ok(v) = self
                .get_json::<Value>(path, &[("Branch", branch.clone())])
                .await
                && v["LectureHalls"].is_object()
            {
                halls = path.clone();
                break;
            }
        }
        let parts: Vec<_> = personal.trim_matches('/').split('/').collect();
        let absence = if parts.len() >= 2 {
            format!("/{}/{}/absence/mark", parts[0], parts[1])
        } else {
            String::new()
        };
        *self.endpoints.write().await = Endpoints {
            personal,
            group: group_path,
            halls,
            calls,
            types,
            absence,
            branch,
            file_hash: candidates.files.first().cloned().unwrap_or_default(),
            homework_hash: candidates.homework.first().cloned().unwrap_or_default(),
            ..Endpoints::default()
        };
        Ok(())
    }

    async fn first_valid<T: DeserializeOwned>(&self, paths: &[String]) -> Option<String> {
        for path in paths {
            if self.get_json::<T>(path, &[]).await.is_ok() {
                return Some(path.clone());
            }
        }
        None
    }

    pub async fn schedule(&self, group: i64, week: Week, scope: &str) -> Result<Vec<ScheduleDay>> {
        match self.platform {
            Platform::Digital => self.digital_schedule(group, week, scope).await,
            Platform::Legacy => self.legacy_schedule(group, week, scope).await,
        }
    }

    async fn digital_schedule(
        &self,
        group: i64,
        week: Week,
        scope: &str,
    ) -> Result<Vec<ScheduleDay>> {
        let week_start = week.start.format("%Y-%m-%d").to_string();
        let result = if scope.starts_with("group:") {
            let group = group.to_string();
            self.call(
                ServerFn::Group,
                &[("groupId", &group), ("weekStart", &week_start)],
            )
            .await?
        } else {
            let function = if scope.starts_with("teacher:") || !self.teacher.is_empty() {
                ServerFn::Teacher
            } else {
                ServerFn::Student
            };
            self.call(function, &[("weekStart", &week_start)]).await?
        };
        if let Some(error) = server_error(&result) {
            return Err(error);
        }
        let (mut days, refs) = digital_week(&result)?;
        self.absorb_references(refs).await;
        // Tasks and webinars are separate per lesson; a teacher's week has its own calls.
        if !scope.starts_with("teacher:") && self.teacher.is_empty() {
            let group = scope.starts_with("group:").then(|| group.to_string());
            self.lesson_extras(&mut days, group.as_deref(), &week_start)
                .await;
        }
        Ok(days)
    }

    /// Fill in the task (text, deadline, files) of lessons flagged `hasHomework` and the
    /// webinar of distance lessons. Extras are best effort: the week stands without them.
    async fn lesson_extras(&self, days: &mut [ScheduleDay], group: Option<&str>, week_start: &str) {
        let wanted: Vec<_> = days
            .iter()
            .enumerate()
            .flat_map(|(d, day)| {
                day.subjects.iter().enumerate().filter_map(move |(i, s)| {
                    let (homework, webinar) =
                        (s.extended_data.has_homework, s.extended_data.remote);
                    (s.extra_data.sheet > 0 && (homework || webinar)).then_some((
                        d,
                        i,
                        s.extra_data.sheet,
                        homework,
                        webinar,
                    ))
                })
            })
            .take(MAX_LESSON_EXTRAS / 2)
            .collect();
        let results = stream::iter(wanted.into_iter().map(|(d, i, journal, homework, webinar)| async move {
            let (task, link) = tokio::join!(
                async {
                    if !homework {
                        return None;
                    }
                    self.lesson_homework(journal, group, week_start)
                        .await
                        .inspect_err(|e| tracing::debug!(journal, error = %e, "lesson task unavailable"))
                        .ok()
                        .flatten()
                },
                async {
                    if !webinar {
                        return None;
                    }
                    self.lesson_webinar(journal, group, week_start)
                        .await
                        .inspect_err(|e| tracing::debug!(journal, error = %e, "lesson webinar unavailable"))
                        .ok()
                        .flatten()
                }
            );
            (d, i, task, link)
        }))
        .buffered(4)
        .collect::<Vec<_>>()
        .await;
        for (d, i, task, link) in results {
            let homework = &mut days[d].subjects[i].extra_data.homework;
            if let Some(task) = task {
                *homework = task;
            }
            if link.is_some() {
                homework.webinar = link;
            }
        }
    }

    async fn lesson_call(
        &self,
        student: ServerFn,
        of_group: ServerFn,
        journal: i64,
        group: Option<&str>,
        week_start: &str,
    ) -> Result<Value> {
        let value = match group {
            Some(group) => {
                self.call_fields(
                    of_group,
                    &[
                        ("groupId", Field::Text(group)),
                        ("weekStart", Field::Text(week_start)),
                        ("journalId", Field::Number(journal)),
                    ],
                )
                .await?
            }
            None => {
                self.call_fields(
                    student,
                    &[
                        ("weekStart", Field::Text(week_start)),
                        ("journalId", Field::Number(journal)),
                    ],
                )
                .await?
            }
        };
        if let Some(error) = server_error(&value) {
            return Err(error);
        }
        Ok(value)
    }

    /// `{homework: {task, deadline, files: [{userFileId, caption, ...}]} | null}`; the group
    /// variant may add `status`, and `restricted` gives `None` like a missing task.
    async fn lesson_homework(
        &self,
        journal: i64,
        group: Option<&str>,
        week_start: &str,
    ) -> Result<Option<Homework>> {
        let value = self
            .lesson_call(
                ServerFn::StudentHomework,
                ServerFn::GroupHomework,
                journal,
                group,
                week_start,
            )
            .await?;
        let homework = &value["homework"];
        if restricted(&value) || !homework.is_object() {
            return Ok(None);
        }
        let text = |key: &str| {
            homework[key]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(Into::into)
        };
        let mut files = Vec::new();
        let mut known = self
            .digital_files
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        for file in homework["files"].as_array().into_iter().flatten() {
            let Some(id) = positive_id(&file["userFileId"]) else {
                continue;
            };
            if file["availability"] != "available" || files.len() >= MAX_ATTACHMENTS {
                continue;
            }
            if known.len() >= MAX_DIGITAL_FILES {
                known.clear();
            }
            known.insert(
                id,
                DigitalFile {
                    caption: file["caption"].as_str().unwrap_or_default().trim().into(),
                    extension: file["extension"].as_str().unwrap_or_default().trim().into(),
                    journal,
                    week_start: week_start.into(),
                    group: group.map(Into::into),
                },
            );
            files.push(id);
        }
        Ok(Some(Homework {
            task: text("task"),
            deadline: text("deadline"),
            webinar: None,
            files,
            lock_upload: None,
        }))
    }

    /// `{webinar: "https://..." | null}` (the group variant may add `status`).
    async fn lesson_webinar(
        &self,
        journal: i64,
        group: Option<&str>,
        week_start: &str,
    ) -> Result<Option<String>> {
        let value = self
            .lesson_call(
                ServerFn::StudentWebinar,
                ServerFn::GroupWebinar,
                journal,
                group,
                week_start,
            )
            .await?;
        Ok((!restricted(&value))
            .then(|| value["webinar"].as_str().map(str::trim))
            .flatten()
            .filter(|s| !s.is_empty())
            .map(Into::into))
    }

    fn digital_file(&self, id: i64) -> Option<DigitalFile> {
        self.digital_files
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&id)
            .cloned()
    }

    async fn legacy_schedule(
        &self,
        group: i64,
        week: Week,
        scope: &str,
    ) -> Result<Vec<ScheduleDay>> {
        self.refresh(self.group.max(group), week, false).await?;
        for attempt in 0..2 {
            let e = self.endpoints.read().await.clone();
            let is_group = scope.starts_with("group:");
            // Never substitute a personal endpoint when the user asks for another group.
            let path = if is_group { &e.group } else { &e.personal };
            ensure!(!path.is_empty(), "group schedule endpoint not found");
            let teacher = if scope.starts_with("teacher:") {
                self.teacher.as_str()
            } else {
                ""
            };
            match self.raw_schedule(path, group, teacher, week).await {
                Ok((days, _)) => return Ok(days),
                Err(e) if attempt == 0 && rediscover(&e) => {
                    self.refresh(self.group.max(group), week, true).await?
                }
                Err(e) => return Err(e),
            }
        }
        bail!("schedule unavailable")
    }

    /// Reference data for rendering. Digital weeks carry their own references, accumulated
    /// in memory; the legacy site serves them from four endpoints.
    pub async fn references(&self) -> Arc<ReferenceData> {
        match self.platform {
            Platform::Digital => self.digital_refs.read().await.clone(),
            Platform::Legacy => Arc::new(self.legacy_references().await),
        }
    }

    /// Start a digital session with the last persisted references, so cached weeks keep their
    /// hall names and lesson times even before this session fetched anything.
    pub async fn seed_references(&self, refs: ReferenceData) {
        if self.platform == Platform::Digital {
            self.absorb_references(refs).await;
        }
    }

    async fn absorb_references(&self, refs: ReferenceData) {
        let mut current = self.digital_refs.write().await;
        let mut merged = ReferenceData::clone(&current);
        if merged.merge(refs) {
            *current = Arc::new(merged);
        }
    }

    async fn legacy_references(&self) -> ReferenceData {
        let e = self.endpoints.read().await.clone();
        let branch_query = [("Branch", e.branch.clone())];
        let (halls, calls, types, absence) = tokio::join!(
            self.get_json::<Value>(&e.halls, &branch_query),
            self.get_json::<Vec<Preset>>(&e.calls, &[]),
            self.get_json::<Vec<PairType>>(&e.types, &[]),
            self.get_json::<Vec<Absence>>(&e.absence, &[])
        );
        let mut refs = ReferenceData::default();
        if let Ok(value) = halls
            && let Some(map) = value["LectureHalls"].as_object()
        {
            for list in map.values() {
                if let Ok(halls) = serde_json::from_value::<Vec<Hall>>(list.clone()) {
                    for h in halls {
                        refs.halls.insert(h.id, h);
                    }
                }
            }
        }
        if let Ok(presets) = calls {
            refs.presets = presets.into_iter().map(|p| (p.id, p)).collect();
        }
        if let Ok(types) = types {
            refs.types = types.into_iter().map(|p| (p.id, p)).collect();
        }
        if let Ok(marks) = absence {
            refs.absence = marks.into_iter().map(|p| (p.digit, p.caption)).collect();
        }
        refs
    }

    async fn auxiliary<T: DeserializeOwned>(
        &self,
        homework: bool,
        suffix: &str,
        query: &[(&str, String)],
    ) -> Result<T> {
        ensure!(
            self.platform == Platform::Legacy,
            "legacy file endpoint unavailable on digital environment"
        );
        for attempt in 0..2 {
            let e = self.endpoints.read().await.clone();
            let hash = if homework {
                &e.homework_hash
            } else {
                &e.file_hash
            };
            let parts: Vec<_> = e.personal.trim_matches('/').split('/').collect();
            if !hash.is_empty() && parts.len() >= 2 {
                let path = format!("/{}/{}/{hash}/{suffix}", parts[0], parts[1]);
                match self.get_json(&path, query).await {
                    Ok(result) => return Ok(result),
                    Err(err) if attempt == 0 && rediscover(&err) => (),
                    Err(err) => return Err(err),
                }
            }
            let c = self.discover(attempt == 0).await?;
            let mut e = self.endpoints.write().await;
            e.file_hash = c.files.first().cloned().unwrap_or_default();
            e.homework_hash = c.homework.first().cloned().unwrap_or_default();
        }
        bail!("file endpoint unavailable")
    }

    pub async fn document(&self, id: i64) -> Result<Document> {
        if self.platform == Platform::Digital {
            let file = self
                .digital_file(id)
                .context("file was not listed by this session")?;
            return Ok(Document {
                id,
                caption: file_name(&file.caption, &file.extension, id),
                icon: file.extension,
            });
        }
        let mut doc: Document = self
            .auxiliary(false, "id", &[("ID", id.to_string())])
            .await?;
        if doc.id == 0 {
            doc.id = id;
        }
        ensure!(doc.id == id, "document ID mismatch");
        if doc.caption.is_empty() {
            doc.caption = format!("file_{id}");
        }
        Ok(doc)
    }

    pub async fn submission(&self, sheet: i64) -> Result<i64> {
        if self.platform == Platform::Digital {
            return Ok(0);
        }
        let v: Value = self
            .auxiliary(true, "homework/check", &[("JournalID", sheet.to_string())])
            .await?;
        Ok(attachment_ids(&v)?.first().copied().unwrap_or(0))
    }

    pub async fn download(&self, id: i64) -> Result<(tempfile::TempPath, String)> {
        if self.platform == Platform::Digital {
            let file = self
                .digital_file(id)
                .context("file was not listed by this session")?;
            let path = match &file.group {
                Some(group) => format!(
                    "/api/timetable/groups/{group}/lessons/{}/files/{id}",
                    file.journal
                ),
                None => format!(
                    "/api/timetable/students/self/lessons/{}/files/{id}",
                    file.journal
                ),
            };
            let mut url = self.base.join(&path)?;
            url.query_pairs_mut()
                .append_pair("weekStart", &file.week_start);
            return self
                .fetch_file(url, file_name(&file.caption, &file.extension, id))
                .await;
        }
        let value: Value = self
            .auxiliary(false, "open", &[("ID", id.to_string())])
            .await?;
        let link = value["Link"]
            .as_str()
            .filter(|s| !s.is_empty())
            .context("file has no download link")?;
        let url = self.base.join(link)?;
        let caption = value["Caption"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(Into::into)
            .unwrap_or_else(|| format!("file_{id}"));
        self.fetch_file(url, caption).await
    }

    /// Stream a file of the site's origin into a temporary file, within `FILE_LIMIT`.
    async fn fetch_file(&self, url: Url, caption: String) -> Result<(tempfile::TempPath, String)> {
        ensure!(
            url.origin() == self.base.origin()
                && url.username().is_empty()
                && url.password().is_none(),
            "file link must use the workspace origin without credentials"
        );
        let (response, _permit) = self
            .send(
                || {
                    self.http
                        .get(url.clone())
                        .timeout(Duration::from_secs(60))
                        .header("Referer", self.base.as_str())
                },
                false,
            )
            .await?;
        ensure_status(&response)?;
        ensure!(
            response
                .content_length()
                .is_none_or(|size| size <= FILE_LIMIT as u64),
            "file exceeds download limit"
        );
        let (file, path) = tempfile::NamedTempFile::new()?.into_parts();
        let mut file = tokio::fs::File::from_std(file);
        let mut stream = response.bytes_stream();
        let mut size = 0usize;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| e.without_url())?;
            size = size.saturating_add(chunk.len());
            ensure!(size <= FILE_LIMIT, "file exceeds download limit");
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        drop(file);
        Ok((path, caption))
    }
}

/// Lesson extras answer without `status` for the own timetable (HAR of 2026-11-03); a
/// `status` other than `available` withholds the content.
fn restricted(value: &Value) -> bool {
    value
        .get("status")
        .is_some_and(|status| status != "available")
}

/// "Отчёт" + "pdf" -> "Отчёт.pdf"; a caption that already ends with the extension stays.
fn file_name(caption: &str, extension: &str, id: i64) -> String {
    let caption = if caption.is_empty() {
        format!("file_{id}")
    } else {
        caption.to_string()
    };
    let extension = extension.trim_start_matches('.');
    if extension.is_empty()
        || caption
            .to_lowercase()
            .ends_with(&format!(".{}", extension.to_lowercase()))
    {
        caption
    } else {
        format!("{caption}.{extension}")
    }
}

fn ensure_status(response: &Response) -> Result<()> {
    if response.status() != StatusCode::OK {
        return Err(ApiError(Some(response.status())).into());
    }
    Ok(())
}
async fn read_body(response: Response, limit: usize) -> Result<Vec<u8>> {
    ensure!(
        response
            .content_length()
            .is_none_or(|size| size <= limit as u64),
        "response exceeds size limit"
    );
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.without_url())?;
        ensure!(
            bytes.len().saturating_add(chunk.len()) <= limit,
            "response exceeds size limit"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
fn sibling(path: &str, name: &str) -> String {
    path.rsplit_once('/')
        .map(|(p, _)| format!("{p}/{name}"))
        .unwrap_or_default()
}
fn has_grade_fields(value: &Value) -> bool {
    match value {
        Value::Object(map) => {
            map.contains_key("Appraisal")
                || map.contains_key("Mark")
                || map.values().any(has_grade_fields)
        }
        Value::Array(values) => values.iter().any(has_grade_fields),
        _ => false,
    }
}
fn lesson_fingerprint(days: &[ScheduleDay]) -> Vec<(String, i64, String, String, i64, String)> {
    let mut result: Vec<_> = days
        .iter()
        .flat_map(|d| {
            d.subjects.iter().map(|s| {
                (
                    d.date.clone(),
                    s.pair,
                    s.discipline.clone(),
                    s.teacher.clone(),
                    s.lecture_hall,
                    s.subgroup.clone(),
                )
            })
        })
        .collect();
    result.sort();
    result
}
pub fn find_group(value: &Value) -> Option<i64> {
    match value {
        Value::Object(map) => map
            .iter()
            .filter(|(k, _)| {
                matches!(
                    k.to_lowercase().replace('_', "").as_str(),
                    "group" | "groupid"
                )
            })
            .find_map(|(_, v)| positive_id(v))
            .or_else(|| map.values().find_map(find_group)),
        Value::Array(values) => values.iter().find_map(find_group),
        _ => None,
    }
}
pub fn find_subgroup(value: &Value) -> Option<&'static str> {
    match value {
        Value::Object(map) => map
            .iter()
            .filter(|(k, _)| k.to_lowercase().contains("subgroup"))
            .find_map(|(_, v)| {
                personal_subgroup(&v.as_str().map(Into::into).unwrap_or_else(|| v.to_string()))
            })
            .or_else(|| map.values().find_map(find_subgroup)),
        Value::Array(values) => values.iter().find_map(find_subgroup),
        _ => None,
    }
}
