# ktk-schedule

<div align="center">

<p>
  <a href="#russian">Русский</a> · <a href="#english">English</a>
</p>

<p>
  <a href="https://go.dev"><img src="https://img.shields.io/badge/Go-1.26.4-00ADD8?style=flat-square&logo=go&logoColor=white" alt="Go 1.26.4"></a>
  <a href="https://core.telegram.org/bots/api"><img src="https://img.shields.io/badge/Telegram-Bot_API-26A5E4?style=flat-square&logo=telegram&logoColor=white" alt="Telegram Bot API"></a>
  <a href="https://sqlite.org"><img src="https://img.shields.io/badge/SQLite-WAL_mode-003B57?style=flat-square&logo=sqlite&logoColor=white" alt="SQLite WAL"></a>
  <a href="https://www.docker.com/"><img src="https://img.shields.io/badge/Docker-Compose-2496ED?style=flat-square&logo=docker&logoColor=white" alt="Docker Compose"></a>
  <a href="https://git.kurumi.world/kurumi/ktk-schedule/src/branch/master/LICENSE"><img src="https://img.shields.io/badge/License-BSD_3--Clause-6f42c1?style=flat-square" alt="BSD-3-Clause"></a>
</p>

<p>
  <img src="https://img.shields.io/badge/platform-linux-lightgrey?style=flat-square&logo=linux&logoColor=white" alt="Linux">
  <img src="https://img.shields.io/badge/CI-Forgejo-FF5733?style=flat-square&logo=gitea&logoColor=white" alt="Forgejo CI">
  <img src="https://img.shields.io/badge/security-AES--GCM-green?style=flat-square&logo=letsencrypt&logoColor=white" alt="AES-GCM">
  <img src="https://img.shields.io/badge/PRs-welcome-0075ca?style=flat-square" alt="PRs welcome">
</p>

</div>

---

<a name="russian"></a>

## Русский

Telegram-бот для workspace КТК. Авторизует пользователей, хранит учётные данные в зашифрованной SQLite, показывает расписание прямо в Telegram и отправляет утренние уведомления.

Собран для ежедневной эксплуатации: HTTP-таймауты, retry-обёртки, rate limiting, circuit breaker, persistent cache, Docker health checks, graceful shutdown.

### Возможности

| Область | Что делает |
|---|---|
| Расписание | Неделя, конкретная дата, сегодня, переключение недель, выбор дня |
| Группы | Своя группа, другая группа, подгруппы, режим обеих подгрупп |
| Преподаватели | Вход как преподаватель и просмотр своего расписания |
| Данные | Время пары, кабинет, тип занятия, статус, оценки, посещаемость |
| Файлы | Вложения к домашним заданиям, загруженные файлы, скачивание по дням |
| Уведомления | Утренняя рассылка расписания по `NOTIFY_TIME` и `TIMEZONE` |
| Надёжность | Ограниченные HTTP-ответы, retry, rate limiting, circuit breaker, cache |
| Эксплуатация | `/health`, `/health/extended`, опциональный `/debug/pprof`, Docker, Forgejo CI/CD |
| Безопасность | AES-GCM для паролей, приватный `/login`, `.env`, SQLite WAL |

### Быстрый старт

```bash
cp .env.example .env
```

Минимально необходимые переменные:

```dotenv
BOT_TOKEN=123456:токен-от-botfather
CREDENTIALS_SECRET=случайная-строка-не-менее-32-символов
```

Сгенерировать секрет:

```bash
openssl rand -base64 32
```

Запустить локально:

```bash
go run ./cmd/bot
```

Запустить в Docker:

```bash
docker compose up --build -d
docker compose logs -f
```

### Команды бота

| Команда | Назначение |
|---|---|
| `/start` | Показать доступные команды |
| `/my_id` | Показать свой Telegram ID |
| `/login логин пароль` | Авторизоваться в workspace |
| `/schedule` | Расписание на текущую неделю |
| `/schedule 01.09` | Расписание на дату (текущий учебный год) |
| `/schedule 2026-09-01` | Расписание на конкретную дату |
| `/notify_on` / `/notify_off` | Включить или выключить утренние уведомления |
| `/announce текст` | Объявление от владельца всем пользователям |
| `reply /announce` | Разослать сообщение из ответа |
| `/stats` | Статистика бота (только владельцу) |

`/login` удаляет сообщение пользователя сразу после получения — логин и пароль не остаются в чате.

### Формат расписания

```
📅 01.06.2026

1 пара [70 мин] — ОП.12 ИКГ
⏰ 08:00-09:10
🔬 Практическое занятие
👤 Бухтоярова Елена Леонидовна
🏫 Кабинет: 301

2 пара [70 мин] — ОП.02 ДМЭМЛ
⏰ 09:20-10:30
📚 Лекция
👤 Сапожникова Елена Владимировна
🏫 Кабинет: 41
```

После `/schedule` доступна inline-навигация: дни, недели, файлы, группы, подгруппы.

### Архитектура

```mermaid
flowchart LR
    T[Telegram] --> A[internal/app]
    A --> K[internal/ktk]
    A --> S[internal/storage]
    A --> TG[internal/tg]
    S --> C[internal/credentials]
    K --> W[Workspace API]
    S --> DB[(SQLite)]
    A --> H[/health/]
```

| Путь | Ответственность |
|---|---|
| `cmd/bot` | Точка входа, загрузка конфига, логирование, graceful shutdown |
| `internal/app` | Telegram-хендлеры, сессии, уведомления, cache, rate limit, health server |
| `internal/ktk` | Workspace-клиент, парсинг, форматирование, файлы, endpoint discovery |
| `internal/storage` | SQLite, миграции, пользователи, зашифрованные credentials, cache |
| `internal/credentials` | AES-GCM шифрование и миграция legacy plaintext |
| `internal/config` | Загрузка и валидация `.env` |
| `internal/tg` | Inline-клавиатуры и compact callback payloads |
| `.gitea/workflows` | CI/CD, сборка образа, deploy, backup, rollback |

Границы пакетов намеренные: Telegram-логика — в `internal/app`, workspace-логика — в `internal/ktk`, хранение — в `internal/storage`.

### Конфигурация

Все переменные описаны в [.env.example](.env.example). Обязательные:

| Переменная | Описание |
|---|---|
| `BOT_TOKEN` | Токен бота от `@BotFather` |
| `CREDENTIALS_SECRET` | Секрет AES-GCM, не менее 32 символов |
| `OWNER_TELEGRAM_ID` | Telegram ID владельца для `/announce` и `/stats` |
| `KTK_BASE_URL` | Базовый URL workspace (по умолчанию `https://workspace.ktk-45.ru`) |
| `KTK_DEVICE_NAME` | Имя устройства для sign-in запросов |
| `DEFAULT_GROUP_ID` | Группа по умолчанию после первого входа |
| `DEFAULT_SUBGROUP` | Подгруппа по умолчанию: `1` или `2` |
| `NOTIFY_TIME` | Время утренних уведомлений, формат `HH:MM` |
| `TIMEZONE` | Часовой пояс, например `Asia/Yekaterinburg` |
| `DATABASE_PATH` | Путь к SQLite-файлу |
| `HEALTH_ADDR` | Адрес health-сервера, по умолчанию `127.0.0.1:8080` |
| `PPROF_ENABLED` | Включить `/debug/pprof` на health-сервере |

### Разработка

Нужны: Go 1.26.4+, [`just`](https://github.com/casey/just), `golangci-lint`, `govulncheck`. Для hot reload — `air`.

```bash
just setup       # подключить .githooks
just setup-air   # установить air
just setup-lint  # установить golangci-lint
just setup-vuln  # установить govulncheck
```

```bash
just dev         # hot reload (air)
just run         # обычный запуск
just test        # go test -count=1 ./...
just race        # тесты с -race
just lint        # golangci-lint run
just vuln        # govulncheck ./...
just build       # собрать ./ktk-schedule
just check       # полная локальная проверка
just ci-check    # проверка уровня CI
just backup      # backup SQLite из Docker volume
just clean       # удалить бинарник, БД и логи
```

### Эксплуатация

Docker Compose запускает сервис с persistent volume для SQLite:

```bash
docker compose up --build -d
docker compose logs -f
```

Health endpoints:

| Endpoint | Назначение |
|---|---|
| `/health` | Базовая проверка процесса |
| `/health/extended` | Расширенная диагностика |
| `/debug/pprof` | Профилирование, только при `PPROF_ENABLED=true` |

Health-сервер и pprof не должны быть открыты наружу без контроля доступа.

Forgejo CI/CD выполняет проверки качества, собирает Docker image, публикует в registry и деплоит на VDS. Перед деплоем создаётся backup SQLite; если новый контейнер не проходит healthcheck — pipeline выполняет rollback на предыдущий image.

### Лицензия

[BSD-3-Clause](./LICENSE)

---

<a name="english"></a>

## English

Telegram bot for the KTK workspace. Authenticates users, stores credentials in encrypted SQLite, renders schedules directly in Telegram, and sends morning notifications.

Built for daily production use: HTTP timeouts, retry wrappers, rate limiting, circuit breaker, persistent cache, Docker health checks, graceful shutdown.

### Features

| Area | What it does |
|---|---|
| Schedule | Week view, specific dates, today, week switching, day selection |
| Groups | Personal group, other group, subgroup selection, both subgroups mode |
| Teachers | Teacher sign-in and schedule retrieval |
| Academic data | Lesson time, room, type, current status, grades, attendance |
| Files | Homework attachments, uploaded files, day-based download |
| Notifications | Morning schedule delivery via `NOTIFY_TIME` and `TIMEZONE` |
| Reliability | Bounded HTTP reads, retries, rate limiting, circuit breaker, cache |
| Operations | `/health`, `/health/extended`, optional `/debug/pprof`, Docker, Forgejo CI/CD |
| Security | AES-GCM password storage, private `/login`, `.env` secrets, SQLite WAL |

### Quick Start

```bash
cp .env.example .env
```

Minimum required variables:

```dotenv
BOT_TOKEN=123456:your-token-from-botfather
CREDENTIALS_SECRET=random-string-at-least-32-characters
```

Generate a secret:

```bash
openssl rand -base64 32
```

Run locally:

```bash
go run ./cmd/bot
```

Run with Docker:

```bash
docker compose up --build -d
docker compose logs -f
```

### Bot Commands

| Command | Purpose |
|---|---|
| `/start` | Show available commands |
| `/my_id` | Show your Telegram ID |
| `/login username password` | Sign in to the workspace |
| `/schedule` | Open the current week schedule |
| `/schedule 01.09` | Schedule for a date in the current academic year |
| `/schedule 2026-09-01` | Schedule for an exact date |
| `/notify_on` / `/notify_off` | Enable or disable morning notifications |
| `/announce text` | Send an owner announcement to all users |
| `reply /announce` | Broadcast the replied-to message |
| `/stats` | Show bot statistics (owner only) |

`/login` deletes the user message immediately after receipt — credentials never stay in the chat.

### Schedule Output

```
📅 01.06.2026

1 пара [70 мин] — ОП.12 ИКГ
⏰ 08:00-09:10
🔬 Практическое занятие
👤 Бухтоярова Елена Леонидовна
🏫 Кабинет: 301

2 пара [70 мин] — ОП.02 ДМЭМЛ
⏰ 09:20-10:30
📚 Лекция
👤 Сапожникова Елена Владимировна
🏫 Кабинет: 41
```

After `/schedule`, inline navigation is available for days, weeks, files, groups, and subgroups.

### Architecture

```mermaid
flowchart LR
    T[Telegram] --> A[internal/app]
    A --> K[internal/ktk]
    A --> S[internal/storage]
    A --> TG[internal/tg]
    S --> C[internal/credentials]
    K --> W[Workspace API]
    S --> DB[(SQLite)]
    A --> H[/health/]
```

| Path | Responsibility |
|---|---|
| `cmd/bot` | Entry point, config loading, logging, graceful shutdown |
| `internal/app` | Telegram handlers, sessions, notifications, cache, rate limit, health server |
| `internal/ktk` | Workspace client, parsing, formatting, files, endpoint discovery |
| `internal/storage` | SQLite, migrations, users, encrypted credentials, schedule cache |
| `internal/credentials` | AES-GCM encryption and legacy plaintext migration |
| `internal/config` | `.env` loading and validation |
| `internal/tg` | Inline keyboards and compact callback payloads |
| `.gitea/workflows` | CI/CD, image build, deploy, backup, rollback |

Package boundaries are intentional: Telegram orchestration in `internal/app`, workspace logic in `internal/ktk`, persistence in `internal/storage`.

### Configuration

All variables are documented in [.env.example](.env.example). Required:

| Variable | Description |
|---|---|
| `BOT_TOKEN` | Telegram bot token from `@BotFather` |
| `CREDENTIALS_SECRET` | AES-GCM secret, at least 32 characters |
| `OWNER_TELEGRAM_ID` | Owner Telegram ID for `/announce` and `/stats` |
| `KTK_BASE_URL` | Workspace base URL (default `https://workspace.ktk-45.ru`) |
| `KTK_DEVICE_NAME` | Device name sent with sign-in requests |
| `DEFAULT_GROUP_ID` | Default group after first sign-in |
| `DEFAULT_SUBGROUP` | Default subgroup: `1` or `2` |
| `NOTIFY_TIME` | Morning notification time, `HH:MM` format |
| `TIMEZONE` | Time zone, e.g. `Asia/Yekaterinburg` |
| `DATABASE_PATH` | Path to the SQLite file |
| `HEALTH_ADDR` | Health server address, default `127.0.0.1:8080` |
| `PPROF_ENABLED` | Enable `/debug/pprof` on the health server |

### Development

Required: Go 1.26.4+, [`just`](https://github.com/casey/just), `golangci-lint`, `govulncheck`. Hot reload uses `air`.

```bash
just setup       # configure .githooks
just setup-air   # install air
just setup-lint  # install golangci-lint
just setup-vuln  # install govulncheck
```

Common commands:

```bash
just dev         # hot reload (air)
just run         # run normally
just test        # go test -count=1 ./...
just race        # tests with -race
just lint        # golangci-lint run
just vuln        # govulncheck ./...
just build       # build ./ktk-schedule
just check       # full local verification before handoff
just ci-check    # CI-level verification
just backup      # backup SQLite from Docker volume
just clean       # remove binary, database, and logs
```

### Operations

Docker Compose starts the service with a persistent SQLite volume:

```bash
docker compose up --build -d
docker compose logs -f
```

Health endpoints:

| Endpoint | Purpose |
|---|---|
| `/health` | Basic process health check |
| `/health/extended` | Extended diagnostics |
| `/debug/pprof` | Profiling, only when `PPROF_ENABLED=true` |

Do not expose the health server or pprof publicly without access control.

Forgejo CI/CD runs quality checks, builds the Docker image, publishes it to the registry, and deploys to the VDS. A compressed SQLite backup is created before deployment; if the new container fails health checks, the pipeline attempts a rollback to the previous image.

### License

[BSD-3-Clause](./LICENSE)
