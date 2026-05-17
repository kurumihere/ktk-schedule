# ktk-schedule

<p align="center">
  <img src="https://cataas.com/cat" width="200" alt="ktk-schedule"/>
</p>

<p align="center">
  <a href="https://go.dev"><img src="https://img.shields.io/badge/Go-1.26+-00ADD8?style=for-the-badge&logo=go&logoColor=white" alt="Go"/></a>
  <a href="https://docker.com"><img src="https://img.shields.io/badge/Docker-ready-2496ED?style=for-the-badge&logo=docker&logoColor=white" alt="Docker"/></a>
  <a href="https://sqlite.org"><img src="https://img.shields.io/badge/SQLite-storage-003B57?style=for-the-badge&logo=sqlite&logoColor=white" alt="SQLite"/></a>
  <a href="https://forgejo.org"><img src="https://img.shields.io/badge/Forgejo%20CI-passing-2ea44f?style=for-the-badge&logo=forgejo&logoColor=white" alt="CI"/></a>
  <br/>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-BSD--3--Clause-blue?style=for-the-badge" alt="License"/></a>
  <a href="https://code.forgejo.org/forgejo/runner"><img src="https://img.shields.io/badge/CD-auto--deploy-2ea44f?style=for-the-badge" alt="CD"/></a>
</p>

<p align="center">
  Telegram bot for KTK schedule — workspace auth, API autodiscovery, subgroup filtering,
  pair timing with countdown, grades and attendance marks, daily notifications.
</p>

<p align="center">
  <a href="#русский">Русский</a> |
  <a href="#english">English</a>
</p>

## Русский

Telegram-бот для просмотра расписания КТК через workspace. Логинится по логину/паролю, сам находит актуальные API-адреса, фильтрует по подгруппам, показывает время пар с обратным отсчётом, оценки и отметки о посещаемости.

### Возможности

| Возможность | Описание |
| --- | --- |
| Авторизация | Вход по логину и паролю от workspace |
| Расписание | По датам и неделям с inline-клавиатурой |
| Группы | Смена через `/group` |
| Подгруппы | 1-я, 2-я или обе сразу |
| Время пар | Длительность, диапазон и обратный отсчёт |
| Оценки и отметки | 2–5 с `+/-`, символы Н/О/Б с причинами |
| Автопоиск API | Находит endpoint-ы сам, включая запасные с оценками |
| Уведомления | Ежедневная рассылка по утрам |
| Объявления | Владелец может разослать сообщение всем |
| Rate limit | 15 сек кулдаун на `/schedule` |
| Шифрование | AES-GCM для паролей в SQLite |
| Health check | HTTP `/health` для Docker |
| Retry | Exponential backoff при сбоях |
| Логирование | `log/slog` с уровнями debug/info/warn/error |
| CI/CD | Forgejo Actions: тесты → сборка → деплой на VDS |

### Команды

| Команда | Назначение |
| --- | --- |
| `/start` | Список команд |
| `/my_id` | Твой Telegram ID |
| `/login логин пароль` | Авторизация |
| `/schedule [дата]` | Расписание на неделю или дату |
| `/group 269` | Сменить группу |
| `/subgroup 1` | Первая подгруппа |
| `/subgroup 2` | Вторая подгруппа |
| `/subgroups_on` | Обе подгруппы |
| `/subgroups_off` | Только свою |
| `/notify_on` | Включить уведомления |
| `/notify_off` | Отключить |
| `/announce текст` | Рассылка (только владелец) |
| `reply /announce` | Разослать ответное сообщение |

### Быстрый старт

```bash
cp .env.example .env
# заполни BOT_TOKEN и CREDENTIALS_SECRET
go run ./cmd/bot
```

### Docker Compose

```bash
docker compose up --build -d
docker compose logs -f ktk-schedule
```

### Конфигурация

Все переменные — в `.env.example`. Обязательные: `BOT_TOKEN`, `CREDENTIALS_SECRET`.

```bash
openssl rand -base64 32  # сгенерировать CREDENTIALS_SECRET
```

### Разработка

**Требования:** Go 1.26+, `just`, `air`, `golangci-lint`.

```bash
just setup       # pre-commit hook
just setup-air   # установить air
just setup-lint  # установить golangci-lint
just dev         # hot-reload
just check       # fmt → vet → test → build
just lint        # golangci-lint
just docker      # docker compose up --build -d
```

Pre-commit hook автоматически форматирует код и прогоняет `go vet`.

### Подгруппы

| API | В боте | Видно |
| --- | --- | --- |
| `left` | 1 подгруппа | Первой подгруппе |
| `right` | 2 подгруппа | Второй подгруппе |
| `middle` | общая | Всем |

### Время пар

```
1 пара [90 мин] — Математика
⏰ 09:00-10:30
⏳ идёт 15 мин, осталось 75 мин
```

### Безопасность

Пароли шифруются AES-256-GCM. Смена `CREDENTIALS_SECRET` требует перелогина всех пользователей.

---

## English

A Telegram bot for KTK schedule. Logs in via workspace, auto-discovers API endpoints, filters by subgroup, shows lesson times with countdown, grades, and attendance marks.

### Features

Sign-in, schedule with inline navigation, group/subgroup switching, pair timing with countdown, grades (2–5 + +/-), attendance marks (Н/О/Б with reasons), API autodiscovery, daily morning notifications, owner announcements, 15s rate limit, AES-GCM encryption, Docker healthcheck, retry with backoff, structured logging, CI/CD with auto-deploy.

### Commands

`/start`, `/my_id`, `/login`, `/schedule`, `/group`, `/subgroup`, `/subgroups_on/off`, `/notify_on/off`, `/announce`.

### Quick Start

```bash
cp .env.example .env
go run ./cmd/bot
```

### Development

```bash
just setup
just dev     # hot-reload
just check   # fmt + vet + test + build
```

### Project Structure

| Path | Purpose |
| --- | --- |
| `cmd/bot` | Entry point |
| `internal/app` | Handlers, sessions, notifications, rate limiter |
| `internal/config` | `.env` loading, validation |
| `internal/ktk` | Workspace client, discovery, schedule formatting |
| `internal/storage` | SQLite with WAL mode, migrations |
| `internal/credentials` | AES-GCM encryption |
| `internal/tg` | Inline keyboards |
| `.air.toml` | Hot-reload config |
| `.golangci.yml` | Linter config |
| `.githooks/` | Pre-commit hook |
| `.gitea/workflows/` | CI |
| `Justfile` | Dev commands |

---

### ⚠️ Disclaimer

This project uses a third-party API. The author is not affiliated with KTK and is not responsible for API changes, downtime, or data format changes.
