# ktk-schedule

[![Go](https://img.shields.io/badge/Go-1.26+-00ADD8?style=flat&logo=go)](https://go.dev)
[![Docker](https://img.shields.io/badge/Docker-ready-2496ED?style=flat&logo=docker)](https://docker.com)
[![SQLite](https://img.shields.io/badge/SQLite-storage-003B57?style=flat&logo=sqlite)](https://sqlite.org)
[![License](https://img.shields.io/badge/License-BSD--3--Clause-blue.svg)](https://opensource.org/licenses/BSD-3-Clause)

Telegram bot for KTK schedule — workspace auth, API autodiscovery, subgroup filtering, pair timing with countdown, grades and attendance marks, daily notifications.

[Русский](#русский) | [English](#english)

## Русский

### О проекте

Бот для просмотра расписания КТК через workspace. Логинится по логину/паролю, сам находит актуальные API-адреса, умеет фильтровать по подгруппам, показывать время пар с обратным отсчётом, оценки и отметки о посещаемости.

### Возможности

| Возможность | Описание |
| --- | --- |
| Авторизация | Вход по логину и паролю от workspace. |
| Расписание | По датам и неделям, с inline-клавиатурой. |
| Группы | Смена группы через `/group`. |
| Подгруппы | 1-я, 2-я или обе сразу. |
| Время пар | Длительность, временной диапазон и обратный отсчёт. |
| Оценки и отметки | Appraisal (2–5) с +/- и символы посещаемости (Н, О, Б). |
| Причины пропусков | Расшифровка причин из `/absence/mark`. |
| Автопоиск API | Находит endpoint-ы сам, включая запасные с оценками. |
| Уведомления | Ежедневная рассылка расписания по утрам. |
| Объявления | Владелец бота может разослать сообщение всем. |
| Шифрование паролей | AES-GCM перед записью в SQLite. |
| Health endpoint | HTTP `/health` для Docker healthcheck. |
| Retry с backoff | Повтор запроса при сбоях с экспоненциальной задержкой. |
| Structured logging | `log/slog` с уровнями debug/info/warn/error. |
| Docker | Готовый docker-compose.yml. |

### Команды

| Команда | Назначение |
| --- | --- |
| `/start` | Показать список команд. |
| `/my_id` | Твой Telegram ID. |
| `/login логин пароль` | Авторизоваться. |
| `/schedule [дата]` | Расписание на неделю или дату. |
| `/group 269` | Сменить группу. |
| `/subgroup 1` | Выбрать первую подгруппу. |
| `/subgroup 2` | Выбрать вторую подгруппу. |
| `/subgroups_on` | Показывать обе подгруппы. |
| `/subgroups_off` | Только свою подгруппу. |
| `/notify_on` | Включить утренние уведомления. |
| `/notify_off` | Отключить. |
| `/announce текст` | Разослать объявление (только владельцу). |
| `reply /announce` | Разослать ответное сообщение. |

### Быстрый старт

```bash
cp .env.example .env
# заполни BOT_TOKEN и CREDENTIALS_SECRET
go run ./cmd/bot
```

### Конфигурация

Пример — `.env.example`.

| Переменная | Обязательная | По умолчанию | Описание |
| --- | --- | --- | --- |
| `BOT_TOKEN` | Да | — | Токен от BotFather. |
| `OWNER_TELEGRAM_ID` | Нет | `0` | Кому доступна `/announce`. |
| `KTK_BASE_URL` | Нет | `https://workspace.ktk-45.ru` | Базовый URL workspace. |
| `DATABASE_PATH` | Нет | `ktk-schedule.db` | Путь к SQLite. |
| `CREDENTIALS_SECRET` | Да | — | Ключ шифрования паролей, минимум 32 символа. |
| `KTK_SIGN_IN_PATH` | Нет | `/sign-in` | Endpoint авторизации. |
| `KTK_DEVICE_NAME` | Нет | `ktk-schedule` | Имя устройства для sign-in. |
| `KTK_SCHEDULE_PATH` | Нет | — | Ручной endpoint расписания (если автопоиск не справляется). |
| `KTK_LECTURE_HALL_PATH` | Нет | — | Ручной endpoint аудиторий. |
| `KTK_CALL_PRESET_PATH` | Нет | — | Ручной endpoint расписания звонков. |
| `KTK_BRANCH_ID` | Нет | — | Branch ID для lecture hall (если автопоиск не нашёл). |
| `KTK_DEBUG_SCHEDULE` | Нет | `false` | Логировать сырую структуру расписания. |
| `DEFAULT_GROUP_ID` | Нет | `269` | Группа после первого `/login`. |
| `DEFAULT_SUBGROUP` | Нет | `1` | Подгруппа (`1` или `2`). |
| `NOTIFY_TIME` | Нет | `07:30` | Время утренней рассылки (`HH:MM`). |
| `TIMEZONE` | Нет | `Asia/Yekaterinburg` | Таймзона. |
| `LOG_LEVEL` | Нет | `info` | Уровень логов: `debug`, `info`, `warn`, `error`. |
| `HEALTH_PORT` | Нет | `8080` | Порт для HTTP `/health`. |

Сгенерировать `CREDENTIALS_SECRET`:

```bash
openssl rand -base64 32
```

### Локальный запуск

```bash
go run ./cmd/bot
```

Сборка:

```bash
go build -trimpath -ldflags="-s -w" -o ktk-schedule ./cmd/bot
```

### Docker Compose

```bash
docker compose up --build -d
```

```bash
docker compose logs -f ktk-schedule
docker compose down
```

### Подгруппы

Workspace отдаёт подгруппу пары в поле `Subgroup`:

| Значение API | В боте | Кому видно |
| --- | --- | --- |
| `left` | 1 подгруппа | Первой подгруппе. |
| `right` | 2 подгруппа | Второй подгруппе. |
| `middle` | общая | Всем. |

По умолчанию — `DEFAULT_SUBGROUP`. Если после логина workspace вернул подгруппу, бот подхватит её сам.

### Время пар

Бот берёт расписание звонков из `call-preset` и пишет для каждой пары:

- Длительность в заголовке: `1 пара [90 мин]`
- Диапазон: `⏰ 09:00-10:30`
- Для сегодняшних пар — обратный отсчёт:
  - `⏳ идёт 15 мин, осталось 75 мин`
  - `⏳ начнётся через 10 мин`

### Оценки и отметки

Под каждой парой бот показывает:

```
📊 Оценка: 4+
📊 Отметка: Н опоздание на занятие
```

**Оценка** — число от 2 до 5, иногда с `+` или `-`.

**Отметка** — символ посещаемости:

| Символ | Что значит |
| --- | --- |
| `Н` | Не было (с причиной, если есть) |
| `Б` | Болел (с причиной) |
| `О` | Опоздал |

Причины подтягиваются из `/absence/mark`.

Некоторые endpoint-ы workspace не отдают `Appraisal`/`Mark`. Бот ищет запасной с оценками. Не нашёл — не показывает, пишет warning в лог.

### Безопасность

Пароли шифруются AES-GCM перед записью в SQLite. Сменишь или потеряешь `CREDENTIALS_SECRET` — придётся перелогиниться всем пользователям.

### Debug режим

Включи `KTK_DEBUG_SCHEDULE=true`, чтобы увидеть в логах сырую структуру расписания:

```text
schedule debug: days=6 subjects=12
schedule debug item day=0: { ... }
```

### Тесты

```bash
go test -count=1 ./...
go vet ./...
go build -o /dev/null ./cmd/bot
```

На Windows вместо `/dev/null` — `NUL`.

### Структура проекта

| Путь | Что там |
| --- | --- |
| `cmd/bot` | Entry point. |
| `internal/app` | Telegram handlers, сессии, нотификации. |
| `internal/config` | Загрузка `.env`, валидация. |
| `internal/ktk` | Клиент workspace, discovery, форматирование расписания. |
| `internal/storage` | SQLite, миграции. |
| `internal/credentials` | Шифрование паролей. |
| `internal/tg` | inline-клавиатуры, helpers. |

### ⚠️ Disclaimer

Проект использует API стороннего сервиса. Автор не связан с КТК и не отвечает за изменения API, недоступность сервиса или формат данных.

## English

### About

A Telegram bot for the KTK schedule. Logs in via workspace, auto-discovers API endpoints, filters by subgroup, shows lesson times with countdown, grades, and attendance marks.

### Features

| Feature | Description |
| --- | --- |
| Sign-in | Workspace login/password. |
| Schedule | By date and week, inline navigation. |
| Groups | Switch with `/group`. |
| Subgroups | 1st, 2nd, or both. |
| Pair timing | Duration, time range, countdown. |
| Grades and marks | Appraisal (2–5) with +/-, attendance symbols (Н, О, Б). |
| Absence reasons | Decoded from `/absence/mark`. |
| API autodiscovery | Finds endpoints, falls back to grade-capable ones. |
| Notifications | Daily morning schedule digest. |
| Announcements | Owner can broadcast to all users. |
| Encrypted passwords | AES-GCM before SQLite. |
| Health endpoint | HTTP `/health` for Docker. |
| Retry with backoff | Exponential backoff on transient errors. |
| Structured logging | `log/slog`, levels debug/info/warn/error. |
| Docker | docker-compose.yml included. |

### Bot Commands

| Command | Description |
| --- | --- |
| `/start` | Show help. |
| `/my_id` | Show your Telegram ID. |
| `/login username password` | Sign in. |
| `/schedule [date]` | Schedule for the week or a date. |
| `/group 269` | Change group. |
| `/subgroup 1` | Select subgroup one. |
| `/subgroup 2` | Select subgroup two. |
| `/subgroups_on` | Show both subgroups. |
| `/subgroups_off` | Show only yours. |
| `/notify_on` | Enable morning notifications. |
| `/notify_off` | Disable. |
| `/announce text` | Broadcast (owner only). |
| `reply /announce` | Broadcast the replied message. |

### Quick Start

```bash
cp .env.example .env
# set BOT_TOKEN and CREDENTIALS_SECRET
go run ./cmd/bot
```

### Configuration

See `.env.example`.

| Variable | Required | Default | Description |
| --- | --- | --- | --- |
| `BOT_TOKEN` | Yes | — | Token from BotFather. |
| `OWNER_TELEGRAM_ID` | No | `0` | Who can use `/announce`. |
| `KTK_BASE_URL` | No | `https://workspace.ktk-45.ru` | Workspace base URL. |
| `DATABASE_PATH` | No | `ktk-schedule.db` | SQLite path. |
| `CREDENTIALS_SECRET` | Yes | — | Encryption key, at least 32 chars. |
| `KTK_SIGN_IN_PATH` | No | `/sign-in` | Sign-in endpoint. |
| `KTK_DEVICE_NAME` | No | `ktk-schedule` | Device name for sign-in. |
| `KTK_SCHEDULE_PATH` | No | — | Manual schedule endpoint (override auto-discovery). |
| `KTK_LECTURE_HALL_PATH` | No | — | Manual lecture hall endpoint. |
| `KTK_CALL_PRESET_PATH` | No | — | Manual call-preset endpoint. |
| `KTK_BRANCH_ID` | No | — | Branch ID for lecture halls (override). |
| `KTK_DEBUG_SCHEDULE` | No | `false` | Log raw schedule item. |
| `DEFAULT_GROUP_ID` | No | `269` | Default group. |
| `DEFAULT_SUBGROUP` | No | `1` | Default subgroup (`1` or `2`). |
| `NOTIFY_TIME` | No | `07:30` | Notification time (`HH:MM`). |
| `TIMEZONE` | No | `Asia/Yekaterinburg` | Timezone. |
| `LOG_LEVEL` | No | `info` | Log level: `debug`, `info`, `warn`, `error`. |
| `HEALTH_PORT` | No | `8080` | HTTP `/health` port. |

Generate `CREDENTIALS_SECRET`:

```bash
openssl rand -base64 32
```

### Local Run

```bash
go run ./cmd/bot
```

Build:

```bash
go build -trimpath -ldflags="-s -w" -o ktk-schedule ./cmd/bot
```

### Docker Compose

```bash
docker compose up --build -d
```

```bash
docker compose logs -f ktk-schedule
docker compose down
```

### Subgroups

Workspace exposes lesson subgroup in the `Subgroup` field:

| API Value | In Bot | Visible to |
| --- | --- | --- |
| `left` | subgroup 1 | Subgroup one users. |
| `right` | subgroup 2 | Subgroup two users. |
| `middle` | common | Everyone. |

Defaults to `DEFAULT_SUBGROUP`. Picks up the user's subgroup from workspace after login if available.

### Pair Timing

The bot reads the call schedule from `call-preset` and shows:

- Duration in the header: `1 пара [90 мин]`
- Time range: `⏰ 09:00-10:30`
- Countdown for today's lessons:
  - `⏳ идёт 15 мин, осталось 75 мин` — in progress
  - `⏳ начнётся через 10 мин` — starts within the hour

### Grades and Marks

The bot shows appraisal and marks below each lesson:

```
📊 Оценка: 4+
📊 Отметка: Н опоздание на занятие
```

**Appraisal** — a number from 2 to 5, possibly with `+` or `-`.

**Mark** — an attendance symbol:

| Symbol | Meaning |
| --- | --- |
| `Н` | Absent (with reason if available) |
| `Б` | Sick (with reason) |
| `О` | Late |

Reasons come from `/absence/mark`.

Some endpoints don't return `Appraisal`/`Mark`. The bot tries a grade-capable fallback. If none found — no grades shown, warning in log.

### Security

Passwords are encrypted with AES-GCM before hitting SQLite. Change or lose `CREDENTIALS_SECRET` and everyone has to `/login` again.

### Debug Mode

Set `KTK_DEBUG_SCHEDULE=true` to dump the raw schedule structure to logs:

```text
schedule debug: days=6 subjects=12
schedule debug item day=0: { ... }
```

### Tests

```bash
go test -count=1 ./...
go vet ./...
go build -o /dev/null ./cmd/bot
```

On Windows replace `/dev/null` with `NUL`.

### Project Structure

| Path | Purpose |
| --- | --- |
| `cmd/bot` | Entry point. |
| `internal/app` | Handlers, sessions, notifications. |
| `internal/config` | `.env` loading, validation. |
| `internal/ktk` | Workspace client, discovery, formatting. |
| `internal/storage` | SQLite, migrations. |
| `internal/credentials` | Password encryption. |
| `internal/tg` | Inline keyboards, helpers. |

### ⚠️ Disclaimer

This project uses a third-party API. The author is not affiliated with KTK and is not responsible for API changes, downtime, or data format changes.

## Cat in the readme 🐈

<p align="center">
    <img src="https://cataas.com/cat" align="center" width="480" />
</p>
