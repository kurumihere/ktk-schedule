# AGENTS.md - ktk-schedule

KTK college schedule bot for Telegram and VK, in Rust (teloxide, sqlx/SQLite, reqwest).
Not a web service, not a library: the only artifact is the `ktk-schedule` binary.
Production talks to the digital platform `https://ktk.education-system.ru/` (TanStack Start
server functions, seroval wire format); the legacy `workspace.ktk-45.ru` REST site is still
supported and covered by tests.

## 0. Layout

- `src/main.rs` - CLI (`--help`, `--version`, `--check`), dotenv, tracing (plain text unless stdout is a terminal), starts `app::run`.
- `src/lib.rs` - only `pub mod`, so `tests/` can reach everything.
- `src/app.rs` - `App`, `Session`, `Loaded`, `LoginRequired`; moka caches, rate limit, circuit breaker, sign-in/out, `load/resolve/show/present`, notifications, `run` (dispatcher, VK loop, two-phase shutdown).
- `src/telegram.rs` - commands, `schedule:*` callbacks, keyboards, `/announce`, file download.
- `src/vk.rs` - VK Bots Long Poll loop, paced API client (`VkClient`), keyboards, handlers.
- `src/workspace.rs` - one signed-in site session (`Workspace`): `Platform`, `Site`, discovery, server functions, legacy REST, files.
- `src/model.rs` - schedule data, `View/Screen/Week`, date helpers, seroval codec, digital week parser, `SessionRecord`, `ReferenceData`, limits.
- `src/render.rs` - **every user-facing text**, schedule formatting, week labels, `fit_text`.
- `src/storage.rs` - SQLite: `accounts`, `sessions`, `schedules`, `views`, `reference_cache`; sealing via `Cipher`.
- `src/credentials.rs` - AES-256-GCM `cache:v1:` sealing bound to a context.
- `src/config.rs` - `Config::load/from_values`; secrets from systemd credentials or env.
- `src/systemd.rs` - `sd_notify` over `$NOTIFY_SOCKET` (READY, WATCHDOG, STOPPING), no libsystemd.
- `src/resilience.rs` - `Circuit` (5 failures -> 30 s cooldown -> one probe).
- `migrations/0001..0005_*.sql` - schema history, applied by `sqlx::migrate!()` in `Storage::open`. 0005 is the current shape.
- `tests/support/mod.rs` - `College` (legacy wiremock site), `Digital` (digital platform mock answering in seroval), `telegram_server()`, `app()`, `digital_app()`, `vk_app()`.
- `tests/*.rs` - integration tests; `tests/har.rs` replays a private HAR (ignored by default).
- `deploy/` - systemd units, `ktk-schedule-deploy`, `ktk-schedule-backup`, `install.sh`, `ctl.sh`, `update.sh` (section 11).
- `.github/` - `ci.yml`, `security.yml`, `dependabot.yml` (auto-merge), `actions/setup-rust` (checks only; deploys come from the workstation).
- `.env.example` - local config template; `.env` is gitignored.

Generated, never committed, never a source of truth: `target/`, `data/`, `*.sqlite3*`, `*.har`.

## 1. Golden rules - never violate

1. **Private chat first.** `/login`, `/logout`, `/schedule`, `/notify_*`, personal buttons, `/stats`, `/announce`: Telegram only in `private()` chats (`chat.is_private() && from.id == chat.id`), VK only when `peer_id == from_id`. In groups the bot ignores everything but its own commands. Telegram inline mode (`telegram::handle_inline`) may post into any chat, so it shows only a `group:` scope through `App::render_shared` / `render::shared_schedule` (no marks, no tasks or files; webinars and live times as of sending). A violation is a P0 bug.
2. **Passwords are never stored and never logged.** `/login` sends the password to the site once (`App::sign_in`) and keeps only the resulting site session (`sessions` table, sealed). No re-login with stored credentials exists or may be added: a rejected session means `LoginRequired` -> the user signs in again. In Telegram the `/login` message is deleted first; if `delete_message` fails, the login is cancelled and nothing is sent anywhere.
3. **HTTPS only, credentials never in URLs.** `KTK_BASE_URL` and `VK_API_URL` must be HTTPS without userinfo (`config.rs:https_url`). Redirects stay same-origin without userinfo, file links must be on the site origin (`workspace.rs:http_client/download`). The loopback-HTTP bypass exists only in `tests/support/mod.rs`. Errors that may carry a URL with a secret (VK long poll key) are logged `without_url()`.
4. **Per-user scopes.** Cache scope is `personal`, `group:{id}` or `teacher:{hash}` (`View::scope`). Another group is always fetched separately; grades/homework from `personal` never leak into a `group:` scope.
5. **Everything stored is sealed.** Sessions, schedules, views and reference data go through `Storage::seal/unseal` (`cache:v1:`, AES-256-GCM, context = `("sessions", id)`, `("schedules", id, scope, week)`, `("views", id, message)`, `("references", id)`). Plaintext in SQLite is forbidden. `CREDENTIALS_SECRET` must stay stable; `Storage::check_secret` refuses a secret that cannot read the saved sessions.
6. **Limits are mandatory:** `MAX_ATTACHMENTS=64`, attachment depth 32 / nodes 4096, `JSON_LIMIT=4 MiB`, `FILE_LIMIT=50 MiB`, seroval `MAX_SEROVAL_NODES=1 Mi` (references cannot amplify), dates within `MAX_DAYS_FROM_TODAY=3650`. Lift a limit only with explicit user approval.
7. **`unsafe_code = "forbid"`** (`Cargo.toml`). No exceptions without explicit permission (this also forbids `std::env::set_var` in tests).
8. **Texts live in `render.rs`** and stay short and neutral: no greetings, no addressing the user, no promotion. VK points to Telegram only with the plain link `render::TELEGRAM_BOT`. `tests/domain.rs:bot_texts_stay_short_neutral_and_free_of_promotion` guards this. Keyboards and the timetable layout are part of the product: change them only on request.
9. **Minimal diff, `cargo fmt` always.** Fix at the breakage point; do not reformat or reshuffle unrelated code.
10. An explicit user instruction overrides any rule except 3 (HTTPS/credentials) and 7 (unsafe); for those, ask twice.

## 2. Naming

Callbacks always start with `schedule:`; data elsewhere is ignored.

| Group | Meaning | Example |
|---|---|---|
| `schedule:week:prev/next/select/page:{-1,1}/open:{millis}/today` | week navigation | `schedule:week:open:1725147600000` |
| `schedule:prev/next/today/refresh` | day navigation | `schedule:refresh` |
| `schedule:day:{index}` | day from `loaded.days` | `schedule:day:3` |
| `schedule:subgroup:left/right/all` | subgroup filter (Telegram, students) | `schedule:subgroup:left` |
| `schedule:subgroup:menu`, `schedule:subgroup:set:{left,right,all}` | VK menu and its filter buttons | `schedule:subgroup:set:all` |
| `schedule:my` | back to the own schedule | `schedule:my` |
| `schedule:group:select` + number + `schedule:back` | another group's schedule | `schedule:group:select` |
| `schedule:download` | files of the day (VK answers with the Telegram link) | `schedule:download` |

Invalid: no `schedule:` prefix, `schedule:day:01x` (ignored in VK, error in Telegram), scope `Group:269` (only `group:269`).

Commands: `/start /my_id /login /logout /schedule [date] /notify_on /notify_off /stats /announce`; VK also accepts `/refresh`, `/my` and the words "Начать", "Обновить", "Моё" in any case. Both platforms parse with `telegram::command`. Help text: `render::HELP` / `render::VK_HELP`. Telegram's "/" menu comes from `render::COMMANDS` (+ `OWNER_COMMANDS` in the owner's chat), set at start by `telegram::set_commands` for private chats only; a new command goes there too.

Inline mode (Telegram only, enable with BotFather `/setinline`): `@bot [group] [subgroup] [date]`, parsed by `telegram::inline_request` (`369`, `369 1`, `369 2 05.11`, `369 1 чт`, `369 обе завтра`; days by name via `model::parse_day`; no group = own group and subgroup). Results are a day picker: the week's other days with pairs on top, the asked day (today by default) last, next to the input field; `is_personal`, cached 60 s; no session -> empty results with the `INLINE_LOGIN` button (`/start login`).

Subgroups: canonical `left/right`; input normalized by `personal_subgroup()` (`1/первая/left -> left`); labels via `subgroup_label()`.

## 3. Minimal change pattern

A handler takes `lock_user(id)`, reads the `View` from storage, mutates it, renders, saves:

```rust
let _guard = app.lock_user(id).await;
let Some(a) = account(app, id).await? else { return Ok(()); };
let Some(mut view) = app.storage.view(id, message.0).await? else { /* stale */ return Ok(()); };
// one match arm on `data`, e.g. view.select_date(...); view.screen = Screen::Schedule;
app.show(&a, &view, Some(message), refresh).await
```

- Early return for stale views, missing accounts and foreign buttons (`callback_inner`, `vk::handle_callback`).
- One new button = one match arm + one line in `schedule_keyboard`/`schedule_keyboard_vk`.
- Errors: `tracing::warn!` + a `render` text. `telegram::failure(&error)` picks `LOGIN_AGAIN` for `LoginRequired`, `SCHEDULE_FAILED` otherwise; `telegram::login_failure` picks `LOGIN_REJECTED` (typed `CredentialsRejected`) or `SITE_DOWN`. Never show `{:?}` to a user, never panic.

## 4. Boundaries

| Logic | Where |
|---|---|
| Dates, weeks, visibility (`week_start`, `shift`, `Week`, `day_date`, `parse_date`, `parse_day`, `within_reach`, `visible_subjects*`, `non_school_day*`, `infer_subgroup`) | `model.rs` |
| seroval encode/decode, digital week parsing | `model.rs` |
| Texts, emoji, declensions, UTF-16 truncation | `render.rs` |
| Site HTTP, discovery, retries, server functions, files | `workspace.rs` |
| Sealing | `credentials.rs` + `Storage::seal/unseal` |
| SQL, migrations | `storage.rs` + `migrations/*.sql` |
| Keyboards, commands, callbacks | `telegram.rs`, `vk.rs` |
| Caches, sessions, circuit, rate limit, background, shutdown | `app.rs` |
| Env and credentials | `config.rs` (`from_values` is pure and testable) |

Do not duplicate: lesson timings only `render::timings`; privacy checks only `private()/owner()` and the VK `peer_id` check; stale-cache logic only `App::load`; Sunday "today = next week" only `App::resolve`.

## 5. Hotspots

Grep the symbol first, then read with offset/limit.

| Path | ~Lines | Owns | Caution |
|---|---|---|---|
| `src/workspace.rs` | 1340 | `Platform/Site/Workspace`, `call/call_once`, `discover/crawl`, legacy `refresh` probing, files | server function ids change on site deploys: rediscovery handles 404; change classification only with `tests/digital.rs` + HAR replay |
| `src/model.rs` | 1250 | schedule types, seroval codec, `digital_week`, `View::scope`, limits | two legacy shapes (Date / DayList) plus digital `days/pages/pairs` and `ordinary/directions` |
| `src/vk.rs` | 1190 | long poll loop, `VkClient` pacing, handlers | inline keyboards: <= 6 rows, <= 10 buttons, labels <= 40 chars (`tests/vk.rs`) |
| `src/app.rs` | 980 | sessions, `load`, `resolve`, `render_schedule_content`, `notify_one`, `run` | caches `sessions/schedules/pending_groups`, permits 12, downloads 3, deadlines 45/120 s |
| `src/telegram.rs` | 730 | commands, callbacks, download, announce | privacy, login `delete_message` guard, owner tools private only |
| `src/render.rs` | 470 | all texts, `schedule`, `fit_text` | Telegram limit is 4096 UTF-16 units; cut by `char`, append `…` |
| `src/storage.rs` | 360 | tables, sealing, prune | single connection, WAL, `synchronous=NORMAL`, `secure_delete=ON`; `save_account` wipes caches |

## 6. Extension points

| Point | Gives | Extend when |
|---|---|---|
| `App::load(&account, &view, refresh)` | days + session + `stale`; SQLite fallback on site failure; `LoginRequired` when the site rejected the session | a new schedule source |
| `App::resolve` | the `(View, Loaded)` actually shown (Sunday -> next week) | anything that shows "today" |
| `Session::references/assets` | references (digital: in memory, legacy: 1 h cache) and attachments | new attachment types |
| `App::session(&account)` | the in-memory or restored site session | never create `Workspace::login` outside `sign_in` |
| `App::allow(id, login)` | rate limit: login 5/60 s (block 300 s), other 8/10 s (block 20 s); VK buttons included | new expensive commands |
| `App::lock_user(id)` | per-user mutex | any account/view mutation |
| `Workspace::call(ServerFn, fields)` / `call_fields` (typed `Field::Number`) | one digital server function with rediscovery | new digital endpoints |

Rule of three: the third copy of logic gets extracted. New features do not change existing screens unless asked.

## 7. Helpers (check before writing new ones)

- `model.rs`: `personal_subgroup`, `subgroup_label`, `week_start`, `shift`, `week_millis`, `Week::new/academic_year`, `academic_year_start`, `within_reach`, `day_date`, `parse_date`, `visible_subjects(_scoped)`, `non_school_day(_scoped)`, `infer_subgroup`, `attachment_ids`, `limited_file_ids`, `parse_schedule`, `parse_digital_schedule`, `digital_week`, `seroval_escape/unescape/request`, `seroval_fields` + `Field`, `decode_seroval`, `null_default`, `positive_id`, `ReferenceData::merge/is_empty`, `PlatformUser`.
- `render.rs`: texts (section 1.8), `signed_in`, `vk_signed_in`, `vk_menu`, `stats`, `announce_done`, `file_failed`, `no_pairs`, `group_header`, `inline_title`, `inline_day`, `shared_header`, `shared_schedule`, `week_label`, `short_week_label`, `short_day`, `schedule`, `duration`, `uptime`, `mark`, `file_icon`, `timings`, `fit_text`.
- `workspace.rs`: `unauthorized`, `CredentialsRejected`, `Platform::detect`, `Site::new`, `script_urls`, `find_group`, `find_subgroup`, `Candidates::add`.
- `telegram.rs`: `schedule_keyboard`, `week_keyboard`, `command`, `inline_request`, `failure`, `login_failure`, `private`, `owner`, `account`.
- `app.rs`: `login_required`, `notification_due`, `App::{now, lock_user, allow, sign_in, session, sign_out, load, resolve, shows_today, render_shared, send, show, show_loaded, present, stats, notify_one}`.
- `systemd.rs`: `notify`, `watchdog`.

## 8. Lifecycle and configuration

`app::run`:
1. `getMe`, `App::new` (opens and migrates SQLite, `check_secret`), dispatcher (`worker_queue_size=16`, polling errors logged as warnings), VK loop, background, watchdog; then `READY=1`.
2. On SIGTERM/SIGINT: `STOPPING=1`; `stop` token ends polling and scheduling; requests already running get 25 s (`DRAIN`); then `app.cancel` aborts the rest; background tasks get 3 s; SQLite closes. systemd's `TimeoutStopSec=35` covers this.

| Task | Owner | Notes |
|---|---|---|
| `telegram::handle_message/callback` | dispatcher | 120 s deadline (`/announce` 1 h), errors answered with `render` texts |
| `telegram::handle_inline` | dispatcher | 10 s deadline (Telegram drops older queries), shares the `allow(id, false)` limit |
| `vk::run` -> `dispatch` | `JoinSet`, 16 at once | 120 s deadline, answers on failure, drained on shutdown |
| `App::background` | 30 s ticker | notifications when `notification_due` (0..120 s window), fan-out 4; cleanup of locks/limits + `prune` every 300 s |
| `systemd::watchdog` | `WATCHDOG_USEC / 3` | only under systemd |
| `App::load` | everyone | moka 5 min cache, circuit, 45 s timeout, `LoginRequired` / SQLite fallback |

| Env | Default | Rule |
|---|---|---|
| `BOT_TOKEN` | required | `digits:secret`; credential `bot_token` wins over env |
| `CREDENTIALS_SECRET` | required, >= 32 bytes | stable forever; credential `credentials_secret` |
| `KTK_BASE_URL` | `https://ktk.education-system.ru/` | HTTPS, host, no userinfo; the host picks `Platform` |
| `DATABASE_PATH` | `data/bot.sqlite3` | set by the unit in production; `:memory:` only in tests |
| `KTK_DEVICE_NAME` | `ktk-schedule` | part of the sign-in fingerprint |
| `DEFAULT_GROUP_ID` | `269` | 1..=100000 |
| `DEFAULT_SUBGROUP` | `1` | fallback when the timetable does not reveal it |
| `OWNER_TELEGRAM_ID` / `OWNER_VK_ID` | `0` | `0` disables `/stats` and `/announce` |
| `NOTIFY_TIME` | `07:30` | `HH:MM` |
| `TIMEZONE` | `Asia/Yekaterinburg` | drives `Week` (digital `weekStart` and legacy millis) |
| `VK_GROUP_TOKEN` | none | credential `vk_group_token`; empty = VK off |
| `VK_GROUP_ID` | none | required with the token, positive |
| `VK_API_URL` | `https://api.vk.com/method/` | HTTPS, no userinfo, normalized to end with `/` |
| `RUST_LOG` | `ktk_schedule=info,teloxide=warn` | observability only |

New variable -> `config.rs` (`from_values` + `load` if secret) + `.env.example` + this table. Never read `std::env` outside `config.rs`/`systemd.rs`.

## 9. Persistence (schema 0005, all tables STRICT)

- `accounts(user_id PK, group_id, personal_subgroup, subgroup, show_all, teacher_hash, notify, notified_date, created_at)` - settings only. `user_id` = Telegram ID, or VK ID + `VK_ID_OFFSET` (10^18).
- `sessions(user_id PK -> accounts, data, updated_at)` - sealed `SessionRecord` (cookie header, session id, endpoints). No row = the user has to `/login`. Rotated cookies are re-saved (`App::persist_cookies`).
- `schedules(user_id, scope, week)`, `views(user_id, message_id)` (VK uses the conversation message id), `reference_cache(user_id)` - sealed caches; `prune` drops views after 30 days and weeks after 90.
- `save_account(account, session)` wipes the user's caches and stores the session in one transaction; `notify`/`notified_date` survive a re-login. `delete_account` cascades.
- `notification_ids(date)` = `notify = 1`, not yet notified on `date`, **and a session exists**.
- Migrations are append-only. Rebuilding a table: rename the old ones first, copy, drop children before parents (a cascade would otherwise hit the copies) - see 0005.

## 10. Gotchas

1. **Digital wire format.** Requests are `{t: node, f: 127, m: []}`; responses are bare seroval nodes. Every string node is escaped like JavaScript (`<` -> `\x3C`, `"` -> `\"`): use `seroval_request`/`decode_seroval`, never raw `{"t":1,"s":...}`. `t:11` (null-prototype dictionaries) are objects.
2. **Sessions slide.** A digital session lives 30 days after its last use (`expiresAt - lastSeen = 30 d`); there is nothing to refresh proactively. Failures come in-band as `{error: {appError: {code: "authRequired"}}}` with HTTP 200 -> `server_error` maps them to 401 -> `LoginRequired` (a refused *group* timetable ends the session only if the personal one is refused too).
3. **Server function ids** are content hashes: a site deploy changes them. `Workspace::call` rediscovers once on 404/410 or a non-JSON answer, and one crawl serves all sessions (`Site` discovery generation). Known-good defaults live in `DEFAULT_DIGITAL_*`; verify them with a fresh HAR (section 11) when the site changes.
4. **Digital tasks, webinars, files.** A digital week only flags `hasHomework`, `billingType` and the auditorium; `Workspace::lesson_extras` fetches the task (`ownStudentLessonHomework` / `groupLessonHomework`) of flagged lessons and the webinar of distance lessons, at most `MAX_LESSON_EXTRAS` calls per week. Their ids are `DEFAULT_DIGITAL_*_HOMEWORK/WEBINAR` (not rediscovered yet). Files download from `/api/timetable/{students/self|groups/{id}}/lessons/{journal}/files/{file}?weekStart=` and only when the session listed them (`digital_files`). Webinars of in-person lessons are not fetched.
5. **Distance days.** For students, a day of distance lessons only is shown (all its pairs) when any pair has a webinar or a task (`visible_subjects_scoped`); otherwise it is empty, i.e. a non-school day everywhere (buttons, text, notifications).
6. **Subgroup.** The digital account does not name it; `App::detect_subgroup` reads the personal timetable (only the own split lessons appear). Inconclusive weeks fall back to `DEFAULT_SUBGROUP`.
7. `delete_message` failure cancels a Telegram login (rule 2). VK cannot delete user messages: `render::vk_signed_in` says the password message should be deleted.
8. `fit_text(text, 4096)` counts UTF-16; byte slicing panics.
9. `Circuit`: always `permit.finish(healthy)`; a rejected session counts as healthy.
10. `week_millis` = Monday 06:00 in `TIMEZONE`; `Week` carries both the date and the millis. Never convert millis back with a hardcoded zone.
11. Dates from users go through `parse_date`/`within_reach`; `%Y` accepts "-262144", which used to panic in `week_label`.
12. VK: the long poll key expires about hourly (`failed: 2`) - renew it and keep `ts`. API calls are paced (60 ms) under the 20/s limit; error 6/10 is retried once. Never log poll URLs.
13. teloxide logs listener errors itself unless `dispatch_with_listener` gets a handler (`run` does this).
14. `storage.prune` and `mark_notified` compare `%Y-%m-%d` strings; change the format everywhere at once.
15. Root must not open the live SQLite file in WAL mode: it would leave root-owned `-wal/-shm` files. Deploy and backup scripts run `sqlite3` as `ktk-schedule`.
16. wiremock `MockGuard` drops did not unmount scoped mocks reliably here: prefer `.up_to_n_times(n)`.

## 11. Workflows

Local checks (always, in this order; the pre-commit hook runs the first two):
```
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
```
HAR replay after a site change (never print HAR contents, they hold grades and the password):
```
KTK_HAR_PATH=/path/to/capture.har cargo test --locked --test har -- --ignored
```
Capture with DevTools -> Network, "Preserve log" and "Disable cache" on, so the main JS bundles are included (discovery is only verified when they are).

**CI/CD.** `ci.yml` (on `debian:13-slim`, the server's OS) and `security.yml` (RustSec, also weekly) check every push and PR on GitHub-hosted runners. Deploys run from the workstation: `deploy/update.sh [test|prod] [--quick]` runs the local checks (`--quick` skips them, never for prod), builds `--release`, refuses a binary whose newest `GLIBC_*` symbol is newer than the server's glibc (Arch is ahead of Debian 13), and pipes it zstd-compressed over `ssh vps` into `/usr/local/sbin/ktk-schedule-deploy <test|prod>` (about 30 s for a warm build). prod only deploys a clean tree. No self-hosted runner exists any more.

**Dependabot.** Weekly cargo (minor/patch grouped, majors one by one), GitHub Actions and `rust-toolchain` updates (`.github/dependabot.yml`). `workflows/dependabot.yml` enables auto-merge (rebase, the commit stays Dependabot's) on its own pull requests; the master ruleset `required checks` requires `check` and `audit` (repository admins bypass, so direct pushes work). Green updates land on master by themselves - `git pull` before work and deploy; red ones stay open as pull requests. The merge itself does not trigger CI again (`GITHUB_TOKEN` pushes never do).

**Host** (Debian 13, systemd 257, SSH alias `vps`):

| Path | What |
|---|---|
| `/etc/systemd/system/ktk-schedule@.service` | instances `prod`, `test`: Type=notify, watchdog, sandbox, `MemoryMax=256M` |
| `/etc/ktk-schedule/<instance>/bot.env` | settings (0600 root) |
| `/etc/ktk-schedule/<instance>/{bot_token,credentials_secret,vk_group_token}.cred` | `systemd-creds` sealed secrets |
| `/opt/ktk-schedule/<instance>/ktk-schedule[.prev]` | binary and the previous one |
| `/var/lib/ktk-schedule/<instance>/bot.sqlite3` | data (owner `ktk-schedule`) |
| `/var/backups/ktk-schedule/<instance>/` | 10 pre-deploy + 14 daily backups (`ktk-schedule-backup@<instance>.timer`) |
| `/usr/local/sbin/ktk-schedule-{deploy,backup}` | tooling, run as root over SSH; `deploy/ctl.sh install` updates them and the units |

`ktk-schedule-deploy` rehearses the migration on a copy of the live database, backs it up, swaps the binary, waits for READY and 15 s of stability, and restores the previous binary and database on any failure. `deploy/install.sh` (root, idempotent) provisions all of the above. `deploy/ctl.sh status|install|logs|follow|start|stop|restart|backup [prod|test]` manages it from a workstation.

Rotate a secret on the host: `printf %s "$NEW" | systemd-creds encrypt --name=bot_token - /etc/ktk-schedule/prod/bot_token.cred && systemctl restart ktk-schedule@prod`. Changing `credentials_secret` makes every saved session unreadable (users must `/login` again).

Agent permissions: read-only commands and the local checks above are always fine. Committing, pushing, deploying, `ssh` changes on the host and dependency changes need the user's go-ahead (dependencies also need a `cargo audit` reason). Never change `rust-toolchain.toml` on your own (Dependabot may).

## 12. Self-maintenance

- New env -> section 8 table + `.env.example`.
- New callback/command -> section 2 + `HELP`/`VK_HELP` if it is a command.
- New text -> `render.rs` and the style test in `tests/domain.rs`.
- New helper -> section 7; third copy -> consolidate.
- New table/column -> a new migration + section 9.
- New pitfall -> section 10 with file/function.
- Limits, timeouts, semaphores, paths changed -> sections 1, 5, 8, 11 in the same commit.
