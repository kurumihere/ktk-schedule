use crate::{
    config::Config,
    credentials::Cipher,
    model::*,
    render::{self, Assets},
    storage::{Account, Storage},
    telegram,
    workspace::{self, Workspace},
};
use anyhow::{Result, anyhow};
use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Tz;
use futures::{StreamExt, stream};
use moka::future::Cache;
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Weak},
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

pub struct Session {
    pub client: Workspace,
    refs: Cache<(), Arc<ReferenceData>>,
    docs: Cache<i64, Document>,
    submissions: Cache<i64, i64>,
}

impl Session {
    fn new(client: Workspace) -> Self {
        Self {
            client,
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
    pub async fn references(&self) -> Arc<ReferenceData> {
        self.refs
            .get_with((), async { Arc::new(self.client.references().await) })
            .await
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
    pub async fn assets(&self, subjects: &[&Subject], teacher: bool, refresh: bool) -> Assets {
        let mut assets = Assets::default();
        if teacher {
            return assets;
        }
        if refresh {
            self.submissions.invalidate_all();
            self.docs.invalidate_all();
        }
        let mut sheets: Vec<_> = subjects
            .iter()
            .map(|s| s.extra_data.sheet)
            .filter(|n| *n > 0)
            .collect();
        sheets.sort_unstable();
        sheets.dedup();
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
        let mut ids: Vec<_> = subjects
            .iter()
            .flat_map(|s| s.extra_data.homework.files.iter().copied())
            .chain(assets.submissions.values().copied())
            .collect();
        ids.sort_unstable();
        ids.dedup();
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
        assets
    }
}

pub struct App {
    pub config: Config,
    pub storage: Storage,
    pub bot: Telegram,
    pub bot_username: String,
    sessions: Cache<i64, Arc<Session>>,
    schedules: Cache<ScheduleKey, Arc<Vec<ScheduleDay>>>,
    permits: Arc<Semaphore>,
    pub downloads: Semaphore,
    limits: Mutex<HashMap<(i64, bool), RateWindow>>,
    circuit: crate::resilience::Circuit,
    user_locks: Mutex<HashMap<i64, Weak<Mutex<()>>>>,
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
        Ok(Self {
            config,
            storage,
            bot,
            bot_username,
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
            permits: Arc::new(Semaphore::new(12)),
            downloads: Semaphore::new(3),
            limits: Mutex::new(HashMap::new()),
            circuit: crate::resilience::Circuit::new(5, Duration::from_secs(30)),
            user_locks: Mutex::new(HashMap::new()),
            started: Instant::now(),
            cancel: CancellationToken::new(),
        })
    }

    pub fn now(&self) -> DateTime<Tz> {
        Utc::now().with_timezone(&self.config.timezone)
    }

    pub async fn lock_user(&self, id: i64) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = {
            let mut locks = self.user_locks.lock().await;
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
        let mut limits = self.limits.lock().await;
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
            Workspace::login(
                self.config.base_url.clone(),
                &self.config.device,
                login,
                password,
                self.permits.clone(),
            ),
        )
        .await?
    }

    pub async fn sign_in(&self, id: i64, login: &str, password: &str) -> Result<Account> {
        let client = self.authenticate(login, password).await?;
        let subgroup = personal_subgroup(&client.subgroup)
            .unwrap_or(&self.config.default_subgroup)
            .to_owned();
        let account = Account {
            id,
            login: login.into(),
            password: password.into(),
            group_id: if client.group > 0 {
                client.group
            } else {
                self.config.default_group
            },
            personal_subgroup: subgroup.clone(),
            subgroup,
            show_all: false,
            teacher_hash: client.teacher.clone(),
            notify: false,
        };
        self.storage.save_account(&account).await?;
        self.schedules
            .invalidate_entries_if(move |(user, _, _), _| *user == id)
            .map_err(|e| anyhow!("{e}"))?;
        self.pending_groups.invalidate(&id).await;
        self.sessions
            .insert(id, Arc::new(Session::new(client)))
            .await;
        Ok(account)
    }

    pub async fn session(&self, account: &Account) -> Result<Arc<Session>> {
        self.sessions
            .try_get_with(account.id, async {
                let client = self.authenticate(&account.login, &account.password).await?;
                Ok::<_, anyhow::Error>(Arc::new(Session::new(client)))
            })
            .await
            .map_err(|e| anyhow!("{e}"))
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
        let mut failure = anyhow!("workspace unavailable");
        for attempt in 0..2 {
            let session = match self.session(account).await {
                Ok(s) => s,
                Err(e) => {
                    failure = e;
                    break;
                }
            };
            let client = &session.client;
            let result = self
                .schedules
                .try_get_with(key.clone(), async {
                    let group = view.group.unwrap_or(account.group_id);
                    let millis = week_millis(view.week(), self.config.timezone)?;
                    let permit = self
                        .circuit
                        .acquire()
                        .ok_or_else(|| anyhow!("workspace is temporarily unavailable"))?;
                    let result = tokio::time::timeout(
                        Duration::from_secs(45),
                        client.schedule(group, millis, &scope),
                    )
                    .await
                    .map_err(anyhow::Error::from)
                    .and_then(|r| r);
                    permit.finish(
                        result.is_ok() || result.as_ref().is_err_and(workspace::unauthorized),
                    );
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
            match result {
                Ok(days) => {
                    // Empty timetables can be published moments later; don't freeze them for five minutes.
                    if days.iter().all(|d| d.subjects.is_empty()) {
                        self.schedules.invalidate(&key).await;
                    }
                    return Ok(Loaded {
                        days,
                        session: Some(session),
                        stale: false,
                    });
                }
                Err(e) if attempt == 0 && workspace::unauthorized(e.as_ref()) => {
                    self.sessions.invalidate(&account.id).await
                }
                Err(e) => {
                    failure = anyhow!("{e}");
                    break;
                }
            }
        }
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
        let loaded = self.load(account, view, refresh).await?;
        if view.date == self.now().date_naive()
            && view.week() == week_start(view.date)
            && !loaded.days.is_empty()
            && !loaded.days.iter().any(|d| {
                day_date(&d.date, self.config.timezone) == Some(view.date)
                    && !non_school_day(d, view, !account.teacher_hash.is_empty())
            })
        {
            let mut next_view = view.clone();
            next_view.week_start = telegram::shift(view.week(), 7)?;
            if let Ok(next) = self.load(account, &next_view, false).await
                && !next.days.is_empty()
            {
                return self
                    .show_loaded(account, &next_view, message, &next, false, "")
                    .await;
            }
        }
        self.show_loaded(account, view, message, &loaded, refresh, "")
            .await
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
        let now = self.now();
        let teacher = !account.teacher_hash.is_empty();
        let selected = loaded
            .days
            .iter()
            .find(|day| day_date(&day.date, self.config.timezone) == Some(view.date));
        let mut file_count = 0;
        let text = match selected {
            Some(day) if !non_school_day(day, view, teacher) => {
                let (refs, assets) = if let Some(session) = loaded.session.as_ref() {
                    let subjects = visible_subjects(day, view, teacher);
                    let (refs, assets) = tokio::join!(
                        session.references(),
                        session.assets(&subjects, teacher, refresh)
                    );
                    (refs, assets)
                } else {
                    (Arc::new(ReferenceData::default()), Assets::default())
                };
                if !teacher {
                    for s in visible_subjects(day, view, teacher) {
                        file_count += s.extra_data.homework.files.len();
                        if assets.submissions.contains_key(&s.extra_data.sheet) {
                            file_count += 1;
                        }
                    }
                }
                render::schedule(day, view, &refs, &assets, teacher, now)
            }
            _ if loaded.days.is_empty() => "На этой неделе нет пар.".into(),
            _ => format!(
                "📅 {}\n\nПар нет. Сегодня не учебный день.",
                view.date.format("%d.%m.%Y")
            ),
        };
        let mut text = format!("{prefix}{text}");
        if let Some(group) = view.group {
            text = format!("📋 Расписание группы {group}\n\n{text}");
        }
        if loaded.stale {
            text =
                format!("Сайт расписания недоступен, показываю сохранённое расписание.\n\n{text}");
        }
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
        Ok(format!(
            "📊 Статистика\n\n👥 Всего пользователей: {users}\n🔔 Уведомлений: {notify}\n💾 Активных сессий: {}\n⏳ Аптайм: {}\n🌐 Таймзона: {}",
            self.sessions.entry_count(),
            render::uptime(self.started.elapsed().as_secs()),
            self.config.timezone
        ))
    }

    async fn notify_one(&self, id: i64, today: NaiveDate) -> Result<()> {
        let _guard = self.lock_user(id).await;
        let Some(account) = self.storage.account(id).await? else {
            return Ok(());
        };
        if !account.notify {
            return Ok(());
        }
        let view = View::own(&account, today);
        let loaded = self.load(&account, &view, false).await?;
        if loaded.days.iter().any(|day| {
            day_date(&day.date, self.config.timezone) == Some(today)
                && !non_school_day(day, &view, !account.teacher_hash.is_empty())
        }) {
            self.show_loaded(
                &account,
                &view,
                None,
                &loaded,
                false,
                "Доброе утро. Расписание на сегодня:\n\n",
            )
            .await?;
        }
        self.storage.mark_notified(id, &today.to_string()).await
    }

    async fn background(self: Arc<Self>) {
        let mut ticker = tokio::time::interval(Duration::from_secs(30));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last_cleanup = Instant::now();
        loop {
            tokio::select! { _ = self.cancel.cancelled() => break, _ = ticker.tick() => () }
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
                    .await
                    .retain(|_, lock| lock.strong_count() > 0);
                self.limits.lock().await.retain(|_, r| {
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

pub async fn run(config: Config) -> Result<()> {
    let http = teloxide::net::default_reqwest_settings()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(60))
        .build()?;
    let bot = Bot::with_client(&config.token, http).throttle(Limits::default());
    let me = bot.get_me().await?;
    tracing::info!(
        username = me.user.username.as_deref().unwrap_or_default(),
        "bot started"
    );
    let app = Arc::new(App::new(config, bot.clone(), me.user.username.unwrap_or_default()).await?);
    let cancel = app.cancel.clone();
    let handler = dptree::entry()
        .branch(Update::filter_message().endpoint(telegram::handle_message))
        .branch(Update::filter_callback_query().endpoint(telegram::handle_callback));
    let mut dispatcher = Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![app.clone()])
        .worker_queue_size(16)
        .build();
    let shutdown = dispatcher.shutdown_token();
    let mut background = tokio::spawn(app.clone().background());
    let signal = async {
        #[cfg(unix)]
        {
            let mut terminate =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("install SIGTERM handler");
            tokio::select! { _ = tokio::signal::ctrl_c() => (), _ = terminate.recv() => () }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
    };
    let dispatch = dispatcher.dispatch();
    tokio::pin!(dispatch);
    tokio::select! {
        _ = &mut dispatch => (),
        _ = signal => {
            tracing::info!("shutting down");
            cancel.cancel();
            let _ = shutdown.shutdown();
            if tokio::time::timeout(Duration::from_secs(25), &mut dispatch).await.is_err() { tracing::warn!("handler shutdown timed out"); }
        }
    }
    cancel.cancel();
    if tokio::time::timeout(Duration::from_secs(3), &mut background)
        .await
        .is_err()
    {
        background.abort();
        let _ = background.await;
    }
    app.storage.close().await;
    Ok(())
}
