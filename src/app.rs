use crate::{
    config::Config,
    credentials::Cipher,
    model::*,
    render::{self, Assets},
    storage::{Account, Storage},
    telegram,
    workspace::{self, Platform, Site, Workspace},
};
use anyhow::{Result, anyhow};
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use chrono_tz::Tz;
use futures::{StreamExt, stream};
use moka::future::Cache;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, PoisonError, Weak},
    time::{Duration, Instant},
};
use teloxide::{
    adaptors::{Throttle, throttle::Limits},
    prelude::*,
    requests::RequesterExt,
    types::{InlineKeyboardMarkup, MessageId},
};
use tokio::sync::{Mutex, Semaphore};
use tokio_util::sync::CancellationToken;

pub type Telegram = Throttle<Bot>;
type ScheduleKey = (i64, String, NaiveDate);

/// The site no longer accepts the account's session. No password is stored, so only the
/// user can sign in again; handlers answer with `render::LOGIN_AGAIN`.
#[derive(Debug)]
pub struct LoginRequired;

impl std::fmt::Display for LoginRequired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the site session ended; the user has to sign in again")
    }
}

impl std::error::Error for LoginRequired {}

pub fn login_required(error: &anyhow::Error) -> bool {
    error.is::<LoginRequired>()
}

/// moka hands every waiting caller one shared error; keep the variants handlers branch on.
fn unshare(error: Arc<anyhow::Error>) -> anyhow::Error {
    if error.is::<LoginRequired>() {
        LoginRequired.into()
    } else if let Some(api) = error.downcast_ref::<workspace::ApiError>() {
        workspace::ApiError(api.0).into()
    } else {
        anyhow!("{error:#}")
    }
}

pub struct Session {
    pub client: Workspace,
    refs: Cache<(), Arc<ReferenceData>>,
    docs: Cache<i64, Document>,
    submissions: Cache<i64, i64>,
    /// Cookie header last written to `sessions`, so rotated cookies get persisted.
    saved_cookies: std::sync::Mutex<String>,
    /// References last written to `reference_cache`; unchanged ones are not rewritten.
    saved_refs: std::sync::Mutex<Option<Arc<ReferenceData>>>,
}

impl Session {
    fn new(client: Workspace, saved_cookies: String) -> Self {
        Self {
            client,
            saved_cookies: std::sync::Mutex::new(saved_cookies),
            saved_refs: std::sync::Mutex::default(),
            refs: Cache::builder()
                .max_capacity(1)
                .time_to_live(Duration::from_secs(3600))
                .build(),
            docs: Cache::builder()
                .max_capacity(256)
                .time_to_live(Duration::from_secs(900))
                .build(),
            submissions: Cache::builder()
                .max_capacity(256)
                .time_to_live(Duration::from_secs(60))
                .build(),
        }
    }
    /// Digital references live in the session itself (cheap, always current); legacy ones cost
    /// four requests, so a successful fetch is reused for an hour and a failed one is not kept.
    pub async fn references(&self) -> Arc<ReferenceData> {
        if self.client.platform() == Platform::Digital {
            return self.client.references().await;
        }
        if let Some(refs) = self.refs.get(&()).await {
            return refs;
        }
        let refs = self.client.references().await;
        if !refs.is_empty() {
            self.refs.insert((), refs.clone()).await;
        }
        refs
    }
    async fn document(&self, id: i64) -> Result<Document> {
        self.docs
            .try_get_with(id, self.client.document(id))
            .await
            .map_err(|e| anyhow!("{e}"))
    }
    async fn submission(&self, sheet: i64) -> Result<i64> {
        self.submissions
            .try_get_with(sheet, self.client.submission(sheet))
            .await
            .map_err(|e| anyhow!("{e}"))
    }
    pub async fn assets(
        &self,
        subjects: &[&Subject],
        teacher: bool,
        refresh: bool,
    ) -> Result<Assets> {
        let mut assets = Assets::default();
        if teacher {
            return Ok(assets);
        }
        if refresh {
            self.submissions.invalidate_all();
            self.docs.invalidate_all();
        }
        let sheets = limited_file_ids(subjects.iter().map(|s| s.extra_data.sheet))?;
        let files = limited_file_ids(
            subjects
                .iter()
                .flat_map(|s| s.extra_data.homework.files.iter().copied()),
        )?;
        let results = stream::iter(
            sheets
                .into_iter()
                .map(|sheet| async move { (sheet, self.submission(sheet).await) }),
        )
        .buffered(4)
        .collect::<Vec<_>>()
        .await;
        for (sheet, result) in results {
            if let Ok(id) = result
                && id > 0
            {
                assets.submissions.insert(sheet, id);
            }
        }
        let ids = limited_file_ids(
            files
                .into_iter()
                .chain(assets.submissions.values().copied()),
        )?;
        let results = stream::iter(
            ids.into_iter()
                .map(|id| async move { self.document(id).await }),
        )
        .boxed()
        .buffered(4)
        .collect::<Vec<_>>()
        .await;
        for doc in results.into_iter().flatten() {
            assets.documents.insert(doc.id, doc);
        }
        Ok(assets)
    }
}

pub struct App {
    pub config: Config,
    /// The college website and its shared request budget (12 concurrent requests).
    pub site: Site,
    pub storage: Storage,
    pub bot: Telegram,
    pub bot_username: String,
    pub vk: Option<Arc<crate::vk::VkClient>>,
    sessions: Cache<i64, Arc<Session>>,
    schedules: Cache<ScheduleKey, Arc<Vec<ScheduleDay>>>,
    pub downloads: Semaphore,
    limits: std::sync::Mutex<HashMap<(i64, bool), RateWindow>>,
    circuit: crate::resilience::Circuit,
    user_locks: std::sync::Mutex<HashMap<i64, Weak<Mutex<()>>>>,
    pub pending_groups: Cache<i64, i32>,
    started: Instant,
    pub cancel: CancellationToken,
}

#[derive(Default)]
struct RateWindow {
    times: VecDeque<Instant>,
    blocked: Option<Instant>,
}

pub struct Loaded {
    pub days: Arc<Vec<ScheduleDay>>,
    pub session: Option<Arc<Session>>,
    pub stale: bool,
}

impl App {
    pub async fn new(config: Config, bot: Telegram, bot_username: String) -> Result<Self> {
        let storage = Storage::open(&config.database_path, Cipher::new(&config.secret)?).await?;
        storage.check_secret().await?;
        let vk = match (&config.vk_group_token, config.vk_group_id) {
            (Some(token), Some(group)) => Some(Arc::new(crate::vk::VkClient::new(
                token.clone(),
                group,
                config.vk_api_url.clone(),
            )?)),
            _ => None,
        };
        let site = Site::new(
            config.base_url.clone(),
            config.device.clone(),
            Arc::new(Semaphore::new(12)),
        );
        Ok(Self {
            config,
            site,
            storage,
            bot,
            bot_username,
            vk,
            sessions: Cache::builder()
                .max_capacity(512)
                .time_to_idle(Duration::from_secs(86400))
                .build(),
            schedules: Cache::builder()
                .max_capacity(2048)
                .time_to_live(Duration::from_secs(300))
                .support_invalidation_closures()
                .build(),
            pending_groups: Cache::builder()
                .max_capacity(512)
                .time_to_idle(Duration::from_secs(900))
                .build(),
            downloads: Semaphore::new(3),
            limits: std::sync::Mutex::default(),
            circuit: crate::resilience::Circuit::new(5, Duration::from_secs(30)),
            user_locks: std::sync::Mutex::default(),
            started: Instant::now(),
            cancel: CancellationToken::new(),
        })
    }

    pub fn now(&self) -> DateTime<Tz> {
        Utc::now().with_timezone(&self.config.timezone)
    }

    pub async fn lock_user(&self, id: i64) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = {
            let mut locks = self
                .user_locks
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(lock) = locks.get(&id).and_then(Weak::upgrade) {
                lock
            } else {
                let lock = Arc::new(Mutex::new(()));
                locks.insert(id, Arc::downgrade(&lock));
                lock
            }
        };
        lock.lock_owned().await
    }

    pub async fn allow(&self, id: i64, login: bool) -> bool {
        let now = Instant::now();
        let (window, max, block) = if login { (60, 5, 300) } else { (10, 8, 20) };
        let mut limits = self.limits.lock().unwrap_or_else(PoisonError::into_inner);
        let rate = limits.entry((id, login)).or_default();
        if rate.blocked.is_some_and(|end| now < end) {
            return false;
        }
        while rate
            .times
            .front()
            .is_some_and(|time| now.duration_since(*time) >= Duration::from_secs(window))
        {
            rate.times.pop_front();
        }
        if rate.times.len() >= max {
            rate.blocked = Some(now + Duration::from_secs(block));
            return false;
        }
        rate.times.push_back(now);
        true
    }

    pub async fn authenticate(&self, login: &str, password: &str) -> Result<Workspace> {
        tokio::time::timeout(
            Duration::from_secs(45),
            Workspace::login(&self.site, login, password),
        )
        .await?
    }

    /// Sign in with the user's credentials. The password reaches the site once and is
    /// dropped here: only the resulting site session is stored.
    pub async fn sign_in(&self, id: i64, login: &str, password: &str) -> Result<Account> {
        let client = self.authenticate(login, password).await?;
        let group_id = if client.group > 0 {
            client.group
        } else {
            self.config.default_group
        };
        let mut this_week = None;
        let subgroup = match personal_subgroup(&client.subgroup) {
            Some(subgroup) => subgroup,
            None if !client.teacher.is_empty() => &self.config.default_subgroup,
            None => {
                let (detected, week) = self.detect_subgroup(&client, group_id).await;
                this_week = week;
                detected.unwrap_or(&self.config.default_subgroup)
            }
        }
        .to_owned();
        let account = Account {
            id,
            group_id,
            personal_subgroup: subgroup.clone(),
            subgroup,
            show_all: false,
            teacher_hash: client.teacher.clone(),
            notify: false,
        };
        let record = client.session_record().await;
        self.end_previous_session(id, &record.session_id).await;
        self.storage.save_account(&account, &record).await?;
        self.schedules
            .invalidate_entries_if(move |(user, _, _), _| *user == id)
            .map_err(|e| anyhow!("{e}"))?;
        self.pending_groups.invalidate(&id).await;
        // Detection already fetched this week's personal timetable: let /schedule reuse it.
        if let Some((week, days)) = this_week
            && account.teacher_hash.is_empty()
            && days.iter().any(|d| !d.subjects.is_empty())
        {
            if let Err(error) = self
                .storage
                .save_schedule(id, "personal", &week.start.to_string(), &days)
                .await
            {
                tracing::warn!(user_id = id, error = %error, "could not persist schedule cache");
            }
            self.schedules
                .insert((id, "personal".into(), week.start), Arc::new(days))
                .await;
        }
        self.sessions
            .insert(id, Arc::new(Session::new(client, record.cookie)))
            .await;
        Ok(account)
    }

    /// The digital site does not name a student's subgroup, but a personal timetable lists
    /// only the student's own split lessons. Looks at this week, then at the next one.
    async fn detect_subgroup(
        &self,
        client: &Workspace,
        group: i64,
    ) -> (Option<&'static str>, Option<(Week, Vec<ScheduleDay>)>) {
        let today = self.now().date_naive();
        let mut this_week = None;
        let detect = async {
            for offset in [0, 7] {
                let week = shift(today, offset).and_then(|d| Week::new(d, self.config.timezone));
                let Ok(week) = week else { break };
                let Ok(days) = client.schedule(group, week, "personal").await else {
                    break;
                };
                let detected = infer_subgroup(&days);
                if offset == 0 {
                    this_week = Some((week, days));
                }
                if detected.is_some() {
                    return detected;
                }
            }
            None
        };
        let detected = tokio::time::timeout(Duration::from_secs(20), detect)
            .await
            .ok()
            .flatten();
        (detected, this_week)
    }

    /// End the account's previous site session, unless the site handed out the same one.
    async fn end_previous_session(&self, id: i64, current: &str) {
        let previous = match self.sessions.get(&id).await {
            Some(session) => Some(session.client.session_record().await),
            None => self.storage.session(id).await.ok().flatten(),
        };
        if let Some(previous) =
            previous.filter(|p| !p.session_id.is_empty() && p.session_id != current)
        {
            self.revoke(id, &previous).await;
        }
    }

    async fn revoke(&self, id: i64, record: &SessionRecord) {
        let revoke = async {
            Workspace::from_session(&self.site, record)?
                .revoke_session()
                .await
        };
        match tokio::time::timeout(Duration::from_secs(15), revoke).await {
            Ok(Ok(())) => (),
            Ok(Err(error)) => {
                tracing::warn!(user_id = id, error = %error, "could not end the site session")
            }
            Err(_) => tracing::warn!(user_id = id, "ending the site session timed out"),
        }
    }

    /// The account's site session: in memory, or restored from the encrypted store. With no
    /// password to sign in again, a missing or unreadable session means `LoginRequired`.
    pub async fn session(&self, account: &Account) -> Result<Arc<Session>> {
        self.sessions
            .try_get_with(account.id, async {
                let record = match self.storage.session(account.id).await {
                    Ok(Some(record)) => record,
                    Ok(None) => return Err(LoginRequired.into()),
                    Err(error) => {
                        tracing::warn!(user_id = account.id, error = %error, "saved session is unreadable");
                        return Err(LoginRequired.into());
                    }
                };
                let client = Workspace::from_session(&self.site, &record).map_err(|error| {
                    tracing::warn!(user_id = account.id, error = %error, "saved session is unusable");
                    LoginRequired
                })?;
                if let Ok(Some(refs)) = self.storage.references(account.id).await {
                    client.seed_references(refs).await;
                }
                Ok::<_, anyhow::Error>(Arc::new(Session::new(client, record.cookie)))
            })
            .await
            .map_err(unshare)
    }

    async fn forget_session(&self, id: i64) {
        tracing::info!(
            user_id = id,
            "site session ended; the user has to sign in again"
        );
        self.sessions.invalidate(&id).await;
        if let Err(error) = self.storage.delete_session(id).await {
            tracing::warn!(user_id = id, error = %error, "could not delete the ended session");
        }
    }

    /// A refused group timetable may be a permission matter; only a refused personal
    /// timetable proves that the session itself is gone.
    async fn session_alive(&self, account: &Account, session: &Session) -> bool {
        let Ok(week) = Week::new(self.now().date_naive(), self.config.timezone) else {
            return true;
        };
        let scope = if account.teacher_hash.is_empty() {
            "personal".to_owned()
        } else {
            format!("teacher:{}", account.teacher_hash)
        };
        let check = session.client.schedule(account.group_id, week, &scope);
        !matches!(
            tokio::time::timeout(Duration::from_secs(20), check).await,
            Ok(Err(error)) if workspace::unauthorized(&error)
        )
    }

    /// Cookies the site rotated must survive a restart, or the restored session is stale.
    async fn persist_cookies(&self, id: i64, session: &Session) {
        let current = session.client.cookie_header();
        if *session.saved_cookies.lock().unwrap() == current {
            return;
        }
        let record = session.client.session_record().await;
        match self.storage.save_session(id, &record).await {
            Ok(()) => *session.saved_cookies.lock().unwrap() = current,
            Err(error) => {
                tracing::warn!(user_id = id, error = %error, "could not persist rotated cookies")
            }
        }
    }

    // The caller holds lock_user, as for login, callbacks and notifications.
    pub async fn sign_out(&self, id: i64) -> Result<()> {
        let record = match self.sessions.get(&id).await {
            Some(session) => Some(session.client.session_record().await),
            None => self.storage.session(id).await.ok().flatten(),
        };
        if let Some(record) = record {
            self.revoke(id, &record).await;
        }
        self.storage.delete_account(id).await?;
        self.sessions.invalidate(&id).await;
        self.pending_groups.invalidate(&id).await;
        self.schedules
            .invalidate_entries_if(move |(user, _, _), _| *user == id)
            .map_err(|e| anyhow!("{e}"))?;
        Ok(())
    }

    pub async fn load(&self, account: &Account, view: &View, refresh: bool) -> Result<Loaded> {
        let scope = view.scope(account);
        let key = (account.id, scope.clone(), view.week());
        if refresh {
            self.schedules.invalidate(&key).await;
        }
        if let Some(days) = self.schedules.get(&key).await {
            return Ok(Loaded {
                days,
                session: self.sessions.get(&account.id).await,
                stale: false,
            });
        }
        let week = view.week().to_string();
        let session = self.session(account).await?;
        let client = &session.client;
        let result = self
            .schedules
            .try_get_with(key.clone(), async {
                let group = view.group.unwrap_or(account.group_id);
                let requested = Week::new(view.week(), self.config.timezone)?;
                let permit = self
                    .circuit
                    .acquire()
                    .ok_or_else(|| anyhow!("workspace is temporarily unavailable"))?;
                let result = tokio::time::timeout(
                    Duration::from_secs(45),
                    client.schedule(group, requested, &scope),
                )
                .await
                .map_err(anyhow::Error::from)
                .and_then(|r| r);
                // A refused session is the user's matter, not a sign of an unhealthy site.
                permit
                    .finish(result.is_ok() || result.as_ref().is_err_and(workspace::unauthorized));
                let days = result?;
                if let Err(error) = self
                    .storage
                    .save_schedule(account.id, &scope, &week, &days)
                    .await
                {
                    tracing::warn!(error=%error, "could not persist schedule cache");
                }
                Ok::<_, anyhow::Error>(Arc::new(days))
            })
            .await;
        let failure = match result {
            Ok(days) => {
                // Empty timetables can be published moments later; don't freeze them for five minutes.
                if days.iter().all(|d| d.subjects.is_empty()) {
                    self.schedules.invalidate(&key).await;
                }
                self.persist_cookies(account.id, &session).await;
                return Ok(Loaded {
                    days,
                    session: Some(session),
                    stale: false,
                });
            }
            Err(error) if workspace::unauthorized(&error) => {
                let personal = scope == "personal" || scope.starts_with("teacher:");
                if personal || !self.session_alive(account, &session).await {
                    self.forget_session(account.id).await;
                    return Err(LoginRequired.into());
                }
                unshare(error)
            }
            Err(error) => unshare(error),
        };
        if let Some(days) = self.storage.schedule(account.id, &scope, &week).await? {
            tracing::warn!(
                account_id = account.id,
                "using saved schedule after workspace failure"
            );
            return Ok(Loaded {
                days: Arc::new(days),
                session: None,
                stale: true,
            });
        }
        Err(failure)
    }

    pub async fn send(&self, id: i64, text: &str) -> Result<Message> {
        Ok(self
            .bot
            .send_message(ChatId(id), render::fit_text(text, 4096))
            .await?)
    }

    pub async fn show(
        &self,
        account: &Account,
        view: &View,
        message: Option<MessageId>,
        refresh: bool,
    ) -> Result<()> {
        let (view, loaded) = self.resolve(account, view, refresh).await?;
        self.show_loaded(account, &view, message, &loaded, refresh, "")
            .await
    }

    /// What to show for `view`. On a Sunday "today" means the coming week, once published.
    pub async fn resolve(
        &self,
        account: &Account,
        view: &View,
        refresh: bool,
    ) -> Result<(View, Loaded)> {
        if view.date == self.now().date_naive()
            && view.date.weekday() == chrono::Weekday::Sun
            && view.week() == week_start(view.date)
        {
            let mut next_view = view.clone();
            next_view.select_date(shift(view.date, 1)?);
            if let Ok(next) = self.load(account, &next_view, refresh).await
                && !next.days.is_empty()
            {
                if let Some(first) = next
                    .days
                    .first()
                    .and_then(|day| day_date(&day.date, self.config.timezone))
                {
                    next_view.select_date(first);
                }
                return Ok((next_view, next));
            }
        }
        Ok((view.clone(), self.load(account, view, refresh).await?))
    }

    /// Reference data survives restarts and outages, but only real and changed data is
    /// worth a write: an empty result must never replace a good cache.
    async fn persist_references(&self, id: i64, session: &Session, refs: &Arc<ReferenceData>) {
        if refs.is_empty()
            || session
                .saved_refs
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_ref()
                .is_some_and(|saved| Arc::ptr_eq(saved, refs))
        {
            return;
        }
        match self.storage.save_references(id, refs).await {
            Ok(()) => {
                *session
                    .saved_refs
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = Some(refs.clone())
            }
            Err(error) => {
                tracing::warn!(user_id = id, error = %error, "could not persist reference cache")
            }
        }
    }

    pub async fn render_schedule_content(
        &self,
        account: &Account,
        view: &View,
        loaded: &Loaded,
        refresh: bool,
        prefix: &str,
    ) -> Result<(String, usize)> {
        let now = self.now();
        let teacher = !account.teacher_hash.is_empty();
        let is_personal = view.scope(account) == "personal";
        let selected = loaded
            .days
            .iter()
            .find(|day| day_date(&day.date, self.config.timezone) == Some(view.date));
        let mut file_count = 0;
        let text = match selected {
            Some(day) if !non_school_day_scoped(day, view, teacher, is_personal) => {
                let subjects = visible_subjects_scoped(day, view, teacher, is_personal);
                let (refs, assets) = if let Some(session) = loaded.session.as_ref() {
                    let (refs, assets) = tokio::join!(
                        session.references(),
                        session.assets(&subjects, teacher, refresh)
                    );
                    // Attachments are extras: the timetable itself must still be shown.
                    let assets = assets.unwrap_or_else(|error| {
                        tracing::warn!(user_id = account.id, error = %error, "attachments unavailable");
                        Assets::default()
                    });
                    self.persist_references(account.id, session, &refs).await;
                    (refs, assets)
                } else {
                    let refs = self
                        .storage
                        .references(account.id)
                        .await
                        .ok()
                        .flatten()
                        .map(Arc::new)
                        .unwrap_or_default();
                    (refs, Assets::default())
                };
                if !teacher {
                    // Exactly the files the download button delivers: unique IDs.
                    file_count =
                        subjects
                            .iter()
                            .flat_map(|s| s.extra_data.homework.files.iter().copied())
                            .chain(subjects.iter().filter_map(|s| {
                                assets.submissions.get(&s.extra_data.sheet).copied()
                            }))
                            .filter(|id| *id > 0)
                            .collect::<HashSet<_>>()
                            .len();
                }
                render::schedule(day, view, &refs, &assets, teacher, is_personal, now)
            }
            _ if loaded.days.is_empty() => render::NO_PAIRS_THIS_WEEK.into(),
            _ => render::no_pairs(view.date),
        };
        let mut text = format!("{prefix}{text}");
        if let Some(group) = view.group {
            text = format!("{}{text}", render::group_header(group));
        }
        if loaded.stale {
            text = format!("{}\n\n{text}", render::STALE_SCHEDULE);
        }
        Ok((text, file_count))
    }

    pub async fn show_loaded(
        &self,
        account: &Account,
        view: &View,
        message: Option<MessageId>,
        loaded: &Loaded,
        refresh: bool,
        prefix: &str,
    ) -> Result<()> {
        let (text, file_count) = self
            .render_schedule_content(account, view, loaded, refresh, prefix)
            .await?;
        let now = self.now();
        let teacher = !account.teacher_hash.is_empty();
        let keyboard = telegram::schedule_keyboard(
            &loaded.days,
            view,
            now.date_naive(),
            self.config.timezone,
            file_count,
            teacher,
        );
        self.present(account.id, message, &text, keyboard, view)
            .await
    }

    pub async fn present(
        &self,
        id: i64,
        message: Option<MessageId>,
        text: &str,
        keyboard: InlineKeyboardMarkup,
        view: &View,
    ) -> Result<()> {
        let text = render::fit_text(text, 4096);
        let message = if let Some(message) = message {
            match self
                .bot
                .edit_message_text(ChatId(id), message, text)
                .reply_markup(keyboard)
                .await
            {
                Ok(_) => (),
                Err(teloxide::RequestError::Api(teloxide::ApiError::MessageNotModified)) => (),
                Err(e) => return Err(e.into()),
            }
            message
        } else {
            self.bot
                .send_message(ChatId(id), text)
                .reply_markup(keyboard)
                .await?
                .id
        };
        self.storage.save_view(id, message.0, view).await?;
        Ok(())
    }

    pub async fn stats(&self) -> Result<String> {
        let (users, notify) = self.storage.counts().await?;
        Ok(render::stats(
            users,
            notify,
            self.sessions.entry_count(),
            self.started.elapsed().as_secs(),
            self.config.timezone,
        ))
    }

    /// Morning message for one subscriber; `background` fans this out.
    pub async fn notify_one(&self, id: i64, today: NaiveDate) -> Result<()> {
        let _guard = self.lock_user(id).await;
        let Some(account) = self.storage.account(id).await? else {
            return Ok(());
        };
        if !account.notify {
            return Ok(());
        }
        let view = View::own(&account, today);
        let user = PlatformUser::from_storage_id(id);
        let loaded = match self.load(&account, &view, false).await {
            Ok(loaded) => loaded,
            // The session ended overnight: say so once. Without a session the account
            // leaves the notification list, so this message is never repeated.
            Err(error) if login_required(&error) => {
                match user {
                    PlatformUser::Telegram(_) => {
                        self.send(id, render::LOGIN_AGAIN).await?;
                    }
                    PlatformUser::Vk(peer) => {
                        if let Some(vk) = self.vk.as_ref() {
                            vk.send_message(peer, render::LOGIN_AGAIN, None).await?;
                        }
                    }
                }
                return self.storage.mark_notified(id, &today.to_string()).await;
            }
            Err(error) => return Err(error),
        };
        let is_personal = view.scope(&account) == "personal";
        if loaded.days.iter().any(|day| {
            day_date(&day.date, self.config.timezone) == Some(today)
                && !non_school_day_scoped(day, &view, !account.teacher_hash.is_empty(), is_personal)
        }) {
            match user {
                PlatformUser::Telegram(_) => {
                    self.show_loaded(&account, &view, None, &loaded, false, "")
                        .await?;
                }
                PlatformUser::Vk(_) => {
                    if let Some(vk) = self.vk.as_ref() {
                        crate::vk::show_vk(self, vk, &account, &view, None, false).await?;
                    }
                }
            }
        }
        self.storage.mark_notified(id, &today.to_string()).await
    }

    /// Morning notifications and cleanup. Stops between ticks on `stop`; a fan-out already
    /// running continues until the hard cancel.
    async fn background(self: Arc<Self>, stop: CancellationToken) {
        let mut ticker = tokio::time::interval(Duration::from_secs(30));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last_cleanup = Instant::now();
        loop {
            tokio::select! { _ = stop.cancelled() => break, _ = ticker.tick() => () }
            let now = self.now();
            if notification_due(now.time(), self.config.notify_time) {
                match self
                    .storage
                    .notification_ids(&now.date_naive().to_string())
                    .await
                {
                    Ok(ids) => {
                        let task = stream::iter(ids.into_iter().map(|id| {
                            let app = &self;
                            async move { if let Err(e) = app.notify_one(id, now.date_naive()).await { tracing::warn!(account_id = id, error = %e, "notification failed"); } }
                        })).boxed().buffer_unordered(4).collect::<Vec<_>>();
                        tokio::select! { _ = self.cancel.cancelled() => break, _ = task => () }
                    }
                    Err(e) => tracing::error!(error = %e, "could not read notification recipients"),
                }
            }
            if last_cleanup.elapsed() >= Duration::from_secs(300) {
                self.user_locks
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .retain(|_, lock| lock.strong_count() > 0);
                self.limits
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .retain(|_, r| {
                        r.times
                            .back()
                            .is_some_and(|t| t.elapsed() < Duration::from_secs(360))
                    });
                if let Err(e) = self.storage.prune().await {
                    tracing::warn!(error = %e, "cache cleanup failed");
                }
                last_cleanup = Instant::now();
            }
        }
    }
}

pub fn notification_due(now: chrono::NaiveTime, target: chrono::NaiveTime) -> bool {
    let delta = now.signed_duration_since(target).num_seconds();
    (0..=120).contains(&delta)
}

/// Grace period for requests already in progress at shutdown; systemd allows 35 s in total.
const DRAIN: Duration = Duration::from_secs(25);

pub async fn run(config: Config) -> Result<()> {
    let http = teloxide::net::default_reqwest_settings()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(60))
        .build()?;
    let bot = Bot::with_client(&config.token, http).throttle(Limits::default());
    let me = bot.get_me().await?;
    let username = me.user.username.unwrap_or_default();
    let app = Arc::new(App::new(config, bot.clone(), username.clone()).await?);
    let handler = dptree::entry()
        .branch(Update::filter_message().endpoint(telegram::handle_message))
        .branch(Update::filter_callback_query().endpoint(telegram::handle_callback));
    let mut dispatcher = Dispatcher::builder(bot.clone(), handler)
        .dependencies(dptree::deps![app.clone()])
        .worker_queue_size(16)
        .build();
    let shutdown = dispatcher.shutdown_token();
    // `stop` ends polling and scheduling; `app.cancel` aborts whatever outlives the drain.
    let stop = CancellationToken::new();
    let background = tokio::spawn(app.clone().background(stop.clone()));
    let vk_task = app
        .vk
        .clone()
        .map(|vk| tokio::spawn(crate::vk::run(app.clone(), vk, stop.clone())));
    let watchdog = tokio::spawn(crate::systemd::watchdog(stop.clone()));
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    // Connection resets on long polling are routine; teloxide retries with backoff.
    let listener = teloxide::update_listeners::polling_default(bot).await;
    let listener_errors = Arc::new(|error: teloxide::RequestError| async move {
        tracing::warn!(%error, "telegram polling failed; retrying");
    });
    let dispatch = dispatcher.dispatch_with_listener(listener, listener_errors);
    tokio::pin!(dispatch);
    tracing::info!(%username, "bot started");
    crate::systemd::notify("READY=1");
    let dispatch_ended = tokio::select! {
        _ = &mut dispatch => { tracing::error!("telegram dispatcher stopped unexpectedly"); true }
        _ = tokio::signal::ctrl_c() => false,
        _ = terminate.recv() => false,
    };
    tracing::info!("shutting down");
    crate::systemd::notify("STOPPING=1");
    stop.cancel();
    let telegram = async {
        if !dispatch_ended {
            // The dispatcher drains its workers while it is being polled.
            let _ = shutdown.shutdown();
            (&mut dispatch).await;
        }
    };
    let drained = async {
        tokio::join!(telegram, async {
            if let Some(task) = vk_task {
                let _ = task.await;
            }
        });
    };
    if tokio::time::timeout(DRAIN, drained).await.is_err() {
        tracing::warn!("requests still running after {DRAIN:?}; cancelling them");
    }
    app.cancel.cancel();
    for task in [background, watchdog] {
        if tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .is_err()
        {
            tracing::warn!("background task did not stop in time");
        }
    }
    app.storage.close().await;
    Ok(())
}
