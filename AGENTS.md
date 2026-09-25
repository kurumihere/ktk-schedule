# AGENTS.md - ktk-schedule

KTK schedule Telegram bot in Rust (teloxide + sqlx/SQLite + reqwest to Workspace).
This is NOT a web service and NOT a library: the only artifact is the `ktk-schedule` binary.

Layout:
- `src/main.rs` - CLI (`--help`, `--version`, `--check`), dotenv, tracing, starts `app::run`.
- `src/lib.rs` - only `pub mod` for reuse in `tests/`.
- `src/app.rs` - `App`, `Session`, moka caches, rate limit, circuit breaker, notification background, teloxide dispatcher.
- `src/telegram.rs` - handlers for `/start /login /logout /schedule /notify_on|off /stats /announce`, `schedule:*` callbacks, keyboards.
- `src/workspace.rs` - Workspace login, JS-endpoint discovery, `schedule/references/document/submission/download`.
- `src/model.rs` - `ScheduleDay/Subject/ExtraData/Homework`, `View/Screen`, `week_start/day_date/visible_subjects`, attachment limits.
- `src/render.rs` - schedule text, `HELP`, `week_label`, `fit_text`, icons, lesson timings.
- `src/storage.rs` - SQLite (`accounts/schedules/views`), encryption via `Cipher`.
- `src/credentials.rs` - AES-256-GCM (`v2:` for passwords, `cache:v1:` for cache).
- `src/config.rs` - `Config::load/from_values`, env validation.
- `src/resilience.rs` - `Circuit` (threshold 5, cooldown 30s in `App::new`).
- `migrations/0001_initial.sql`, `0002_encrypted_cache.sql` - schema source of truth, applied via `sqlx::migrate!()`.
- `tests/support/mod.rs` - `College` (wiremock Workspace), `telegram_server()`, `app()`; other `tests/*.rs` are integration tests.
- `deploy/ktk-schedule.service`, `deploy/update.sh`, `deploy/receive.sh` - systemd and VDS deploy.
- `.env.example` - config example; `.env` - local secret (gitignored); prod uses `/etc/ktk-schedule/bot.env`.

Generated: `target/`, `data/*.sqlite*` - never commit, never read as source of truth.

## 1. Golden rules - never violate

1. Private chat first: `/login`, `/logout`, `/schedule`, `/notify_*`, personal buttons - only `message.chat.is_private()`. Group chats get a refusal. A violation is a P0 bug.
2. Never log the `/login` password and never send it to Workspace if `delete_message` failed - login is cancelled (see `telegram.rs:message_inner`).
3. `KTK_BASE_URL` must be HTTPS without userinfo; redirects only same-origin without credentials; file links only from the Workspace origin (`workspace.rs:login/download/request`, `config.rs:from_values`). The HTTPS bypass for loopback wiremock exists only in `tests/support/mod.rs:app` - never move it into prod code.
4. Personal cache is strictly per-user: scope `personal` / `group:{id}` / `teacher:{hash}` (`model.rs:View::scope`). Always fetch another group separately, never mix grades/homework from personal scope into group scope. Mixing scopes is forbidden.
5. Passwords (`v2:` + AAD=telegram_id) and cache (`cache:v1:` + AAD=`("schedules"|"views",...)`) only via `credentials::Cipher`. Plaintext in SQLite is forbidden. `CREDENTIALS_SECRET` must be stable across restarts (otherwise all cache/passwords fail to decrypt).
6. Limits are mandatory: `MAX_ATTACHMENTS=64`, `JSON_LIMIT=4MiB`, `FILE_LIMIT=50MiB`, attachment depth 32, nodes 4096 (`model.rs`, `workspace.rs`). Lift a limit only with explicit user approval.
7. `unsafe_code = "forbid"` (`Cargo.toml:[lints.rust]`). `unsafe` is forbidden without explicit permission.
8. Minimal diff: fix at the breakage point, do not refactor neighbors, do not reformat чужой код (others' lines). `cargo fmt` is mandatory, but no manual reshuffling.
9. An explicit user override lifts any rule except 7 (unsafe) and 3 (HTTP/credentials in prod) - for those, ask twice.

## 2. Naming and grouping

Callbacks: `schedule:` prefix + colon-separated action.

| Group | Meaning | Example |
|---|---|---|
| `schedule:week:prev/next/select/page:{-1,1}/open:{millis}/today` | week navigation | `schedule:week:open:1725147600000` |
| `schedule:prev/next/today/refresh` | day navigation | `schedule:refresh` |
| `schedule:day:{index}` | day selection from `loaded.days` | `schedule:day:3` |
| `schedule:subgroup:left/right/all` | subgroup filter (students) | `schedule:subgroup:left` |
| `schedule:my` | back to own schedule | `schedule:my` |
| `schedule:group:select` + numeric input + `schedule:back` | чужой группы (another group's) view | `schedule:group:select` |
| `schedule:download` | download day files | `schedule:download` |

Invalid: callback without `schedule:` (ignored in `handle_callback`), `schedule:day:01x` (parse error), scope like `Group:269` (only `group:269`).

Commands: `/start /my_id /login /logout /schedule [date] /notify_on /notify_off /stats /announce`. The `HELP` text lives in `render.rs:HELP` - change it there, not in `telegram.rs`.

Subgroups: canon `left/right`, input normalized by `personal_subgroup()` (`1/первая/left -> left`, `2/вторая/right -> right`), display via `subgroup_label()`.

## 3. Minimal change pattern

A handler holds `lock_user(id)`, reads `View` from storage, mutates it, calls `present/show`, saves:

```rust
let _guard = app.lock_user(id).await;
let Some(a) = account(app, id).await? else { return Ok(()); };
let Some(mut view) = app.storage.view(id, message.0).await? else { /* stale */ return Ok(()); };
// ... one match arm on data, e.g. view.select_date(...); view.screen = Screen::Schedule;
app.show(&a, &view, Some(message), refresh).await
```

Rules:
- Early return for stale/guard (`account is None`, `view is None`, чужой callback - someone else's callback) - see `callback_inner`.
- Do not reindent the whole `match` for one arm.
- Add a new button as one arm + one line in `schedule_keyboard`, without rewriting the keyboard.
- Workspace error -> `tracing::warn!` + "try later", never panic and never show `{:?}` to the user.

## 4. Boundaries

Inline up to ~5-7 lines stays in `telegram.rs/app.rs`; more - extract.

| Logic | Where |
|---|---|
| Date/week/visibility parsing (`week_start`, `day_date`, `parse_date`, `visible_subjects`, `non_school_day`) | `model.rs` |
| Message text, emoji, declensions, UTF-16 truncation | `render.rs` (`schedule`, `fit_text`, `week_label`) |
| Workspace HTTP, discovery, retries, JSON parsing | `workspace.rs` |
| Encryption/decryption | `credentials.rs` (`encrypt/decrypt/encrypt_cache/decrypt_cache`) |
| SQL and migrations | `storage.rs` + `migrations/*.sql` |
| Keyboards and commands/callbacks | `telegram.rs` |
| Moka caches, circuit, rate limit, background | `app.rs` |
| Env validation | `config.rs` (`from_values` - pure, testable without env) |

Do not duplicate: lesson timings - only `render::timings`; privacy check - `private()/owner()` in `telegram.rs`; stale-cache check - `App::load`.

## 5. Hotspot files table

Never read big files top to bottom: grep the symbol first, then `read` with offset/limit.

| Path | ~Lines | Owns | Caution |
|---|---|---|---|
| `src/workspace.rs` | 780 | login/discover/schedule/download, regex `API/SCRIPTS/FILE/HOMEWORK/SCHEDULE_LOADER/BRANCH` | discovery is fragile to Workspace markup; change regex only with a test in `tests/workspace.rs` |
| `src/telegram.rs` | 730 | commands, `handle_message/handle_callback/callback_inner/download/announce` | privacy and the login `delete_message` guard; callbacks check ownership |
| `src/app.rs` | 660 | `App/Session/Loaded`, `load/show/show_loaded/present`, `background`, `run` | caches `sessions/schedules/pending_groups`, semaphores `permits=12/downloads=3`, timeouts 45/120s |
| `src/model.rs` | 440 | schedule structs, `attachment_ids/limited_file_ids/parse_schedule`, `View::scope` | limits 64/4096/32; two schedule formats (Date / DayList+Pairs+Subgroups) |
| `src/render.rs` | 330 | day formatting, `HELP`, `fit_text` UTF-16 4096 | Telegram limit is 4096 UTF-16 units, cut by `char`, append `…` |
| `src/storage.rs` | 230 | `accounts/schedules/views`, `prune` | pool `max_connections=1`, WAL, `PRAGMA wal_checkpoint(TRUNCATE)`; `save_account` wipes schedules+views |
| `src/config.rs` | 85 | all env (`BOT_TOKEN`, `CREDENTIALS_SECRET`, `KTK_BASE_URL`, `DATABASE_PATH`, ...) | `from_values` - the spot for unit tests without env |

Before touching a hotspot check existing helpers: `render::fit_text/week_label/short_day`, `model::week_millis/day_date/shift` (shift is in `telegram.rs`), `workspace::unauthorized`.

## 6. Shared extension points

| Point | What it gives | When to extend |
|---|---|---|
| `App::load(&account, &view, refresh)` | days + `session` + `stale`, SQLite fallback on 5xx | new schedule source goes here, not into the handler |
| `Session::assets(&subjects, teacher, refresh)` | `Assets{documents, submissions}` with dedup and `buffered(4)` | new attachment types go here |
| `App::show/show_loaded/present` | render + keyboard + `save_view` | new screen means a new `Screen`, rendered via `show*` |
| `App::session(&account)` | moka Workspace session (idle TTL 24h) | never create `Workspace::login` bypassing it |
| `App::allow(id, login)` | rate limit: login 5/60s block 300s; normal 8/10s block 20s | new expensive commands go through `allow` |
| `App::lock_user(id)` | per-user `Mutex` via `Weak` + cleanup every 300s | any view/account mutation goes under the lock |
| `storage::{account,view,schedule}` | persistent state reads | handlers never keep View in memory between messages |
| `telegram::{schedule_keyboard, week_keyboard, shift, command}` | keyboards and parsing | new buttons go here |

Rule of three: the third copy of login/render/parsing logic gets extracted into shared code. No-op by default: a new feature does not change existing screen text/keyboard without asking.

## 7. Helpers

All helpers live next to their owner, there is no `utils.rs`. Check before creating:

- `model.rs`: `personal_subgroup`, `subgroup_label`, `week_start`, `week_millis`, `day_date`, `parse_date`, `visible_subjects`, `non_school_day`, `attachment_ids`, `limited_file_ids`, `parse_schedule`, `null_default`, `positive_id`.
- `render.rs`: `week_label`, `short_day`, `schedule`, `duration`, `uptime`, `mark`, `file_icon`, `timings`, `fit_text`, `HELP`.
- `workspace.rs`: `unauthorized`, `rediscover`, `script_urls`, `find_group`, `find_subgroup`, `Candidates::add`.
- `telegram.rs`: `button`, `schedule_keyboard`, `week_keyboard`, `shift`, `command`, `private`, `owner`, `account`, `announce`, `download`.
- `app.rs`: `notification_due`, `App::{now, lock_user, allow, authenticate, sign_in, session, sign_out, load, send, show, present, stats}`.
- `credentials.rs`: `Cipher::{new, encrypt, decrypt, encrypt_cache, decrypt_cache}`.

Naming: snake_case functions, `schedule:*` callbacks, `schedule:week:*` week subgroup.

## 8. Lifecycle bus and config pattern

Central dispatcher - `app::run`: `Dispatcher::builder` + `dptree::entry` with two branches (`filter_message -> handle_message`, `filter_callback_query -> handle_callback`), `worker_queue_size=16`, graceful shutdown on SIGTERM/SIGINT + `CancellationToken` + `dispatcher.shutdown_token()` (handler timeout 25s, background 3s).

Background - `App::background`: 30s ticker, notifications when `notification_due(now, notify_time)` (window 0..120s), fan-out `buffer_unordered(4)`, cleanup of `user_locks/limits` + `storage.prune()` every 300s.

| Method | Caller | Purpose |
|---|---|---|
| `handle_message/message_inner` | dispatcher | commands; 120s timeout, error -> "try later" |
| `handle_callback/callback_inner` | dispatcher | buttons; ownership check + `answer_callback_query`; 120s timeout |
| `App::load` | `show/download/notify_one/callback` | 2 attempts (2nd after session invalidation on 401/403), SQLite fallback, `stale` flag |
| `App::show_loaded` | `show/notify_one/callback` | day selection, `refs+assets` via `join!`, group/stale prefixes |
| `notify_one` | `background` | `View::own(today)`, skip non-school days, `mark_notified` |
| `Workspace::refresh` | `schedule` | personal/group endpoint selection, `force` on `rediscover` (404/410) |
| `Workspace::discover` | `login/refresh/auxiliary` | parsing `/` + JS (up to 64 scripts, batches of 4), cache in `candidates` |

Config: `Config::from_values(get)` - pure function; `load()` - env wrapper. Add a new variable to both + `.env.example` + here (table below). Access from another layer goes via `app.config.*`, never read `std::env` in handlers (typical mistake).

| Env | Default | Rule |
|---|---|---|
| `BOT_TOKEN` | required `id:secret` | digits + `:` + no whitespace |
| `CREDENTIALS_SECRET` | required, trimmed len >= 32 | stable; rotation breaks decryption |
| `KTK_BASE_URL` | `https://workspace.ktk-45.ru/` | HTTPS + host + no userinfo |
| `DATABASE_PATH` | `data/bot.sqlite3` | `:memory:` only in tests |
| `KTK_DEVICE_NAME` | `ktk-schedule` | sent to `/sign-in` as `Device` |
| `DEFAULT_GROUP_ID` | `269` | 1..=100000 |
| `DEFAULT_SUBGROUP` | `1` | parsed by `personal_subgroup` |
| `OWNER_TELEGRAM_ID` | `0` | `0` = announce/stats off |
| `NOTIFY_TIME` | `07:30` | `HH:MM` |
| `TIMEZONE` | `Asia/Yekaterinburg` | `chrono-tz::Tz` |
| `RUST_LOG` | `ktk_schedule=info,teloxide=warn` | observability only |

## 9. Persistence, settings, strings, assets

- SQLite via sqlx, single connection (`max_connections=1`), WAL, `secure_delete=ON`, `busy_timeout=5s`. Migrations in `migrations/*.sql` are append-only, applied by `sqlx::migrate!()` in `Storage::open`. Manual `ALTER` outside migrations is forbidden.
- `accounts`: password encrypted `v2:`, AAD=telegram_id. `login/group_id/personal_subgroup/subgroup/show_all/teacher_hash/notify` are open. `save_account` deletes the user's schedules+views (account switch = clean cache). `delete_account` cascade-cleans schedules+views (FK).
- `schedules(telegram_id,scope,week)`: day JSON, `cache:v1:` cipher, context `("schedules",id,scope,week)`. `views(telegram_id,message_id)`: `View` JSON, context `("views",id,message)`. `prune`: views older than 30 days, schedules older than 90 days.
- User settings: `subgroup/show_all` (`set_subgroup`), `notify` (`set_notify`), `notified_date` (`mark_notified`). Notifications: `notification_ids(date)` = `notify=1 AND notified_date<>date`.
- Strings: all user-facing text in Russian lives in `render.rs`. Do not scatter magic strings; pair-type emoji in `render::pair_type`, file icons in `file_icon`.
- Homework assets: `assets.submissions(sheet->file_id)` TTL 60s, `docs(file_id->Document)` TTL 256/900s, `refs` TTL 1h. Teacher (`teacher_hash != ""`) gets no assets. Download: `downloads` semaphore 3, stream into `tempfile::NamedTempFile`, 50MiB limit, `PrivateTmp=true` in the unit.

## 10. Gotchas and pitfalls

1. `delete_message` after `/login` failed -> login is cancelled, password goes nowhere. Do not "fix" by retrying without deletion - this protects the password from staying in chat history.
2. Private chat is mandatory: `private()` checks `chat.is_private() && from.id == chat.id`; callbacks check `query.from.id == chat.id`. чужой кнопки test (someone else's button) - "personal buttons are only for the owner".
3. `View::scope` decides whose grades you see: `group:{id}` for another group, `teacher:{hash}` for a teacher, `personal` otherwise. Mix it up and you leak grades/homework.
4. Empty days (`pair_type==9 || lecture_type==9`) are `non_school_day`, skipped in `notify_one` and shown as "not a school day". An empty week is not frozen in the 5-minute cache (`App::load` invalidates it).
5. `fit_text(text, 4096)` counts UTF-16 units, cuts by `char`, appends `…`. Byte-slicing the string panics/corrupts unicode, forbidden.
6. Workspace discovery: endpoints are found in HTML+JS (`SCHEDULE_LOADER` wins over `API`), `personal` is picked by `(has_grades, subject_count)`, group by a `group_aware` probe of neighbor groups. Breaks on site redesign - fix regex + `tests/workspace.rs`, do not hardcode paths.
7. `Circuit`: 5 failures -> 30s cooldown, single probe. Always call `permit.finish(healthy)`; `unauthorized` counts as healthy (does not trip the circuit). A lost `finish` = stuck probe.
8. `week_millis` = Monday 06:00 local time in `TIMEZONE`. Changing `TIMEZONE` shifts the requested week.
9. `Config::from_values` trims and substitutes defaults on empty strings. Empty `BOT_TOKEN` without `:` is a "BOT_TOKEN is required" error, not a panic.
10. `storage.prune` and `mark_notified(date=%Y-%m-%d)` rely on date strings; change the format everywhere at once (`notify_one`, `background`, `storage.rs` tests).
11. `announce`: owner only (`OWNER_TELEGRAM_ID>0` and equal to sender), text or reply; fan-out `buffer_unordered(4)` via `send_message/copy_message`. Empty DB -> "nobody to send to".
12. Deploy: systemd `TimeoutStopSec=35` > 25s handler + 3s background; `receive.sh` checks `--version` and `NRestarts==0`. Binary over 100MiB or empty is rejected.

## 11. Workflows

User by hand: `cp .env.example .env` + fill `BOT_TOKEN` (test bot), `CREDENTIALS_SECRET` (`openssl rand -base64 32`), `cargo run -- --check`, `cargo run`.

Allowed for the agent (read-only + local checks):
- `cargo fmt --all -- --check`
- `cargo clippy --locked --all-targets -- -D warnings`
- `cargo test --locked --all-targets`
- `cargo build --locked --release` (only if a binary was requested)
- `./target/debug/ktk-schedule --help | --version | --check`
- `cargo audit --deny unsound --deny yanked` (if cargo-audit is installed)
- `rg/grep/glob/read` over code; do NOT run `gh workflow run deploy.yml` without asking

Forbidden for the agent without explicit permission: commit/push, change `.env`/`/etc/ktk-schedule/bot.env`, touch `data/*.sqlite*`, run the prod bot with the prod token, deploy (`deploy/update.sh`, `sudo`, `systemctl`), change `rust-toolchain.toml` and the Rust version (1.98.1), add dependencies without a `cargo audit` reason.

Check order after a fix: `cargo fmt`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked --all-targets`. Tests need loopback wiremock network, no external calls. The pre-commit hook (`.githooks/pre-commit`) runs fmt+clippy.

## 12. Self-maintenance

- New env -> Config tables (section 8) + `.env.example`.
- New callback/command -> Naming table (section 2) + `HELP` if it is a command.
- New shared helper -> section 7; third copy -> consolidate.
- New lifecycle method (background/fan-out/cache) -> section 8 table.
- New SQLite table/column -> `migrations/` new file + section 9.
- New pitfall from a session -> section 10 numbered item with file:line.
- Tribal knowledge rots: when limits/timeouts/semaphores change, update sections 4/8/10 in the same commit.
