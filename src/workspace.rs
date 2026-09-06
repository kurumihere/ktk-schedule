use crate::model::*;
use anyhow::{Context, Result, bail, ensure};
use chrono::Datelike;
use futures::{StreamExt, stream};
use regex::Regex;
use reqwest::{Client as HttpClient, Response, StatusCode, Url};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::{
    collections::{HashSet, VecDeque},
    sync::{Arc, LazyLock},
    time::Duration,
};
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, RwLock, Semaphore};

const JSON_LIMIT: usize = 4 * 1024 * 1024;
pub const FILE_LIMIT: usize = 50 * 1024 * 1024;

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
}

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
    if !value.is_empty() && !list.iter().any(|s| s == value) {
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
            if dest.len() < 128 {
                unique(dest, p);
            }
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
    }
}

pub fn script_urls(text: &str, base: &Url) -> Vec<Url> {
    let mut seen = HashSet::new();
    SCRIPT_SRC
        .captures_iter(text)
        .chain(SCRIPTS.captures_iter(text))
        .filter_map(|c| base.join(&c[1]).ok())
        .filter(|u| u.origin() == base.origin() && seen.insert(u.clone()))
        .take(128)
        .collect()
}

pub struct Workspace {
    http: HttpClient,
    base: Url,
    permits: Arc<Semaphore>,
    endpoints: RwLock<Endpoints>,
    candidates: Mutex<Option<Candidates>>,
    refresh_lock: Mutex<()>,
    pub subgroup: String,
    pub group: i64,
    pub teacher: String,
    branch: String,
}

impl Workspace {
    pub async fn login(
        base: Url,
        device: &str,
        login: &str,
        password: &str,
        permits: Arc<Semaphore>,
    ) -> Result<Self> {
        let origin = base.origin();
        let http = HttpClient::builder()
            .cookie_store(true)
            .user_agent("ktk-schedule/2.0")
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::custom(move |attempt| {
                if attempt.previous().len() >= 5 {
                    attempt.error("too many redirects")
                } else if attempt.url().origin() != origin {
                    attempt.stop()
                } else {
                    attempt.follow()
                }
            }))
            .build()?;
        let mut this = Self {
            http,
            base,
            permits,
            endpoints: RwLock::new(Endpoints::default()),
            candidates: Mutex::new(None),
            refresh_lock: Mutex::new(()),
            subgroup: String::new(),
            group: 0,
            teacher: String::new(),
            branch: String::new(),
        };
        let permit = this.permits.acquire().await?;
        let response = this
            .http
            .post(this.base.join("/sign-in")?)
            .header("Origin", this.base.origin().ascii_serialization())
            .header("Referer", this.base.as_str())
            .json(&json!({"Login": login, "Password": password, "Device": device}))
            .send()
            .await
            .map_err(|e| e.without_url())?;
        ensure_status(&response)?;
        let body = read_body(response, JSON_LIMIT).await?;
        drop(permit);
        if let Ok(value) = serde_json::from_slice::<Value>(&body) {
            this.subgroup = find_subgroup(&value).unwrap_or_default().into();
            this.group = find_group(&value).unwrap_or(0);
        }
        let candidates = this.discover(false).await?;
        let mut info_paths = candidates.info.clone();
        for path in candidates.schedules.iter().chain(candidates.calls.iter()) {
            unique(&mut info_paths, &sibling(path, "info"));
        }
        let mut info = None;
        for path in info_paths {
            if let Ok(value) = this.get_json::<Value>(&path, &[]).await
                && value["IsStudent"].is_boolean()
            {
                info = Some(value);
                break;
            }
        }
        let info = info.context("could not determine workspace account type")?;
        if let Some(group) = find_group(&info) {
            this.group = group;
        }
        if let Some(subgroup) = find_subgroup(&info) {
            this.subgroup = subgroup.into();
        }
        this.branch = info["Branch"].as_str().unwrap_or_default().into();
        if info["IsStudent"] == false {
            this.teacher = info["Hash"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or("teacher")
                .into();
        }
        Ok(this)
    }

    pub async fn discover(&self, force: bool) -> Result<Candidates> {
        let mut cache = self.candidates.lock().await;
        if !force && let Some(c) = cache.as_ref() {
            return Ok(c.clone());
        }
        let root = self.base.join("/")?;
        let html = self.get_text(root.clone()).await?;
        let mut candidates = Candidates::default();
        candidates.add(&html);
        let mut queue: VecDeque<_> = script_urls(&html, &root).into();
        let mut seen = HashSet::new();
        while !queue.is_empty() && seen.len() < 64 {
            let mut batch = Vec::new();
            while batch.len() < 4 && seen.len() < 64 {
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
        *cache = Some(candidates.clone());
        Ok(candidates)
    }

    async fn request(&self, url: Url, limit: usize) -> Result<Vec<u8>> {
        for attempt in 0..3 {
            let permit = self.permits.acquire().await?;
            let response = self
                .http
                .get(url.clone())
                .header("Referer", self.base.as_str())
                .header("Accept", "application/json,text/html,*/*")
                .header("X-Requested-With", "XMLHttpRequest")
                .send()
                .await;
            let result = match response {
                Ok(response) => {
                    if response.status().is_server_error() && attempt < 2 {
                        drop(response);
                        drop(permit);
                        tokio::time::sleep(Duration::from_millis(100 << attempt)).await;
                        continue;
                    }
                    ensure_status(&response)?;
                    read_body(response, limit).await
                }
                Err(e) if (e.is_connect() || e.is_timeout()) && attempt < 2 => {
                    drop(permit);
                    tokio::time::sleep(Duration::from_millis(100 << attempt)).await;
                    continue;
                }
                Err(e) => Err(e.without_url().into()),
            };
            return result;
        }
        bail!("workspace retries exhausted")
    }

    async fn get_text(&self, url: Url) -> Result<String> {
        Ok(String::from_utf8(self.request(url, JSON_LIMIT).await?)?)
    }

    async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T> {
        ensure!(!path.is_empty(), "workspace endpoint unavailable");
        let mut url = self.base.join(path)?;
        ensure!(
            url.origin() == self.base.origin(),
            "API endpoint has a different origin"
        );
        url.query_pairs_mut()
            .extend_pairs(query.iter().map(|(k, v)| (*k, v.as_str())));
        let bytes = self.request(url, JSON_LIMIT).await?;
        serde_json::from_slice(&bytes).map_err(|_| ApiError(None).into())
    }

    async fn raw_schedule(
        &self,
        path: &str,
        group: i64,
        teacher: &str,
        week: i64,
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
            ("Week", week.to_string()),
        ];
        if !teacher.is_empty() {
            let now = chrono::Utc::now();
            query.push((
                "Year",
                (now.year() - i32::from(now.month() < 9)).to_string(),
            ));
        }
        let value: Value = self.get_json(path, &query).await?;
        let has_grades = has_grade_fields(&value);
        let days = parse_schedule(&value).map_err(|_| ApiError(None))?;
        Ok((days, has_grades))
    }

    pub async fn refresh(&self, group: i64, week: i64, force: bool) -> Result<()> {
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
                            && fingerprint(&other_days) != fingerprint(&days)
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
                            && fingerprint(&other_days) != fingerprint(&days)
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

    pub async fn schedule(&self, group: i64, week: i64, scope: &str) -> Result<Vec<ScheduleDay>> {
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

    pub async fn references(&self) -> ReferenceData {
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
        let v: Value = self
            .auxiliary(true, "homework/check", &[("JournalID", sheet.to_string())])
            .await?;
        Ok(attachment_ids(&v).first().copied().unwrap_or(0))
    }
    pub async fn download(&self, id: i64) -> Result<(tempfile::TempPath, String)> {
        let value: Value = self
            .auxiliary(false, "open", &[("ID", id.to_string())])
            .await?;
        let link = value["Link"]
            .as_str()
            .filter(|s| !s.is_empty())
            .context("file has no download link")?;
        let url = self.base.join(link)?;
        ensure!(
            matches!(url.scheme(), "http" | "https"),
            "invalid file link scheme"
        );
        let _permit = self.permits.acquire().await?;
        let response = self
            .http
            .get(url)
            .timeout(Duration::from_secs(60))
            .header("Referer", self.base.as_str())
            .send()
            .await
            .map_err(|e| e.without_url())?;
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
        let caption = value["Caption"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(Into::into)
            .unwrap_or_else(|| format!("file_{id}"));
        Ok((path, caption))
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
fn fingerprint(days: &[ScheduleDay]) -> Vec<(String, i64, String, String, i64, String)> {
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
