# ktk-schedule

![Go](https://img.shields.io/badge/Go-1.26+-00ADD8?style=flat&logo=go)
![Docker](https://img.shields.io/badge/Docker-ready-2496ED?style=flat&logo=docker)
![SQLite](https://img.shields.io/badge/SQLite-storage-003B57?style=flat&logo=sqlite)
![License](https://img.shields.io/badge/License-BSD--3--Clause-blue.svg)

Telegram bot for KTK schedule access with dynamic workspace API discovery, encrypted credential storage, subgroup filtering, and daily notifications.

[Русский](#русский) | [English](#english)

## Русский

### О проекте

`ktk-schedule` это Telegram-бот для просмотра расписания КТК через workspace. Бот авторизуется от имени пользователя, получает расписание, умеет работать с подгруппами; актуальные API пути находятся автоматически через HTML/JS workspace.

### Возможности

| Возможность | Описание |
| --- | --- |
| Авторизация | Вход через workspace логин и пароль. |
| Расписание | Просмотр расписания по датам и неделям с inline-навигацией. |
| Группы | Пользователь может сменить группу командой `/group`. |
| Подгруппы | Поддержка 1/2 подгруппы и режима показа обеих подгрупп. |
| Уведомления | Ежедневная утренняя отправка расписания. |
| Объявления | Владелец бота может отправлять объявления всем пользователям. |
| Dynamic API discovery | Бот сам находит актуальные workspace API endpoint-ы. |
| Encrypted storage | Пароли сохраняются в SQLite в зашифрованном виде. |
| Docker | Готовый запуск через Docker Compose. |

### Команды бота

| Команда | Назначение |
| --- | --- |
| `/start` | Показать список команд. |
| `/my_id` | Показать твой Telegram ID. |
| `/login логин пароль` | Авторизоваться в workspace. |
| `/schedule [дата]` | Показать расписание на текущую неделю или указанную дату. |
| `/group 269` | Изменить группу. |
| `/subgroup 1` | Выбрать первую подгруппу. |
| `/subgroup 2` | Выбрать вторую подгруппу. |
| `/subgroups_on` | Показывать обе подгруппы в одном расписании. |
| `/subgroups_off` | Показывать только выбранную подгруппу. |
| `/notify_on` | Включить утренние уведомления. |
| `/notify_off` | Отключить утренние уведомления. |
| `/announce текст` | Отправить текстовое объявление всем пользователям. |
| `reply /announce` | Отправить всем пользователям сообщение, на которое дан ответ. |

### Быстрый старт

```bash
cp .env.example .env
go run ./cmd/bot
```

Перед запуском обязательно заполни `BOT_TOKEN` и `CREDENTIALS_SECRET` в `.env`.

### Конфигурация

Пример `.env` находится в `.env.example`.

| Переменная | Обязательная | Значение по умолчанию | Описание |
| --- | --- | --- | --- |
| `BOT_TOKEN` | Да | `null` | Telegram bot token из BotFather. |
| `OWNER_TELEGRAM_ID` | Нет | `0` | Telegram ID владельца, которому доступна команда `/announce`. |
| `KTK_BASE_URL` | Нет | `https://workspace.ktk-45.ru` | Базовый URL workspace. |
| `DATABASE_PATH` | Нет | `ktk-schedule.db` | Путь к SQLite базе. |
| `CREDENTIALS_SECRET` | Да | `null` | Секрет для шифрования паролей, минимум 32 символа. |
| `KTK_SIGN_IN_PATH` | Нет | `/sign-in` | Endpoint авторизации workspace. |
| `KTK_DEVICE_NAME` | Нет | `ktk-schedule` | Имя устройства для sign-in запроса. |
| `KTK_DEBUG_SCHEDULE` | Нет | `false` | Логировать структуру первого элемента расписания. |
| `DEFAULT_GROUP_ID` | Нет | `269` | Группа по умолчанию после первого `/login`. |
| `DEFAULT_SUBGROUP` | Нет | `1` | Подгруппа по умолчанию: `1` или `2`. |
| `NOTIFY_TIME` | Нет | `07:30` | Время утреннего уведомления в формате `HH:MM`. |
| `TIMEZONE` | Нет | `Asia/Yekaterinburg` | Таймзона для выбора текущего дня и уведомлений. |

Сгенерировать `CREDENTIALS_SECRET` можно так:

```bash
openssl rand -base64 32
```

### Локальный запуск

```bash
go run ./cmd/bot
```

Для сборки:

```bash
go build -trimpath -ldflags="-s -w" -o ktk-schedule ./cmd/bot
```

### Docker Compose

```bash
docker compose up --build -d
```

Логи:

```bash
docker compose logs -f ktk-schedule
```

Остановка:

```bash
docker compose down
```

### Подгруппы

Workspace отдаёт подгруппу пары в поле `Subgroup`.

| Значение API | Значение в боте | Поведение |
| --- | --- | --- |
| `left` | `1 подгруппа` | Показывается пользователям первой подгруппы. |
| `right` | `2 подгруппа` | Показывается пользователям второй подгруппы. |
| `middle` | `общая` | Показывается всем пользователям. |

По умолчанию бот использует `DEFAULT_SUBGROUP`. Если workspace отдаёт подгруппу пользователя после авторизации, бот попробует определить её автоматически.

### Безопасность

Пароли пользователей не хранятся открытым текстом. Перед записью в SQLite пароль шифруется через AES-GCM, а ключ строится из `CREDENTIALS_SECRET`.

Важно: если поменять или потерять `CREDENTIALS_SECRET`, ранее сохранённые пароли нельзя будет расшифровать. Пользователям потребуется заново выполнить `/login`.

### Debug режим

Для диагностики структуры расписания можно временно включить:

```env
KTK_DEBUG_SCHEDULE=true
```

После команды `/schedule` в логах появится краткая сводка:

```text
schedule debug: days=6 subjects=12
schedule debug item day=0: { ... }
```

### Тесты и проверки

```bash
go test -count=1 ./...
go vet ./...
go build -o NUL ./cmd/bot
```

На Linux/macOS вместо `NUL` используй `/dev/null`:

```bash
go build -o /dev/null ./cmd/bot
```

### Структура проекта

| Путь | Назначение |
| --- | --- |
| `cmd/bot` | Entry point приложения. |
| `internal/app` | Telegram handlers, sessions, notifications. |
| `internal/config` | Загрузка `.env` и валидация конфигурации. |
| `internal/ktk` | Workspace client, discovery API endpoint-ов, форматирование расписания. |
| `internal/storage` | SQLite persistence и миграции. |
| `internal/credentials` | Шифрование и расшифровка сохранённых паролей. |
| `internal/tg` | Telegram UI helpers, inline keyboards. |

### ⚠️ Disclaimer

Проект использует API стороннего сервиса. Автор не связан с КТК и не несёт ответственности за изменения workspace API, недоступность сервиса или изменение формата данных.

## English

### About

`ktk-schedule` is a Telegram bot for viewing the KTK schedule via the workspace. The bot logs in on the user's behalf, retrieves the schedule, and supports subgroups; the current API endpoints are automatically detected via the workspace's HTML/JS interface.

### Features

| Feature | Description |
| --- | --- |
| Workspace sign-in | Uses the user's workspace credentials. |
| Schedule navigation | Shows schedules by date and week with inline navigation. |
| Group selection | Users can change their group with `/group`. |
| Subgroups | Supports subgroup 1, subgroup 2, and combined view. |
| Daily notifications | Sends the daily schedule at the configured time. |
| Announcements | Bot owner can broadcast announcements to all users. |
| Dynamic API discovery | Discovers current workspace API endpoints automatically. |
| Encrypted storage | Stores passwords encrypted in SQLite. |
| Docker | Ready to run with Docker Compose. |

### Bot Commands

| Command | Description |
| --- | --- |
| `/start` | Show available commands. |
| `/my_id` | Show your Telegram ID. |
| `/login username password` | Sign in to workspace. |
| `/schedule [date]` | Show the current week schedule or a specific date. |
| `/group 269` | Change group. |
| `/subgroup 1` | Select subgroup one. |
| `/subgroup 2` | Select subgroup two. |
| `/subgroups_on` | Show both subgroups in one schedule. |
| `/subgroups_off` | Show only the selected subgroup. |
| `/notify_on` | Enable morning notifications. |
| `/notify_off` | Disable morning notifications. |
| `/announce text` | Send a text announcement to all users. |
| `reply /announce` | Send the replied-to message to all users. |

### Quick Start

```bash
cp .env.example .env
go run ./cmd/bot
```

Before starting the bot, set `BOT_TOKEN` and `CREDENTIALS_SECRET` in `.env`.

### Configuration

The example environment file is available at `.env.example`.

| Variable | Required | Default | Description |
| --- | --- | --- | --- |
| `BOT_TOKEN` | Yes | `null` | Telegram bot token from BotFather. |
| `OWNER_TELEGRAM_ID` | No | `0` | Telegram owner ID allowed to use `/announce`. |
| `KTK_BASE_URL` | No | `https://workspace.ktk-45.ru` | Workspace base URL. |
| `DATABASE_PATH` | No | `ktk-schedule.db` | SQLite database path. |
| `CREDENTIALS_SECRET` | Yes | `null` | Password encryption secret, at least 32 characters. |
| `KTK_SIGN_IN_PATH` | No | `/sign-in` | Workspace sign-in endpoint. |
| `KTK_DEVICE_NAME` | No | `ktk-schedule` | Device name sent with sign-in requests. |
| `KTK_DEBUG_SCHEDULE` | No | `false` | Log the first raw schedule item for diagnostics. |
| `DEFAULT_GROUP_ID` | No | `269` | Default group after the first `/login`. |
| `DEFAULT_SUBGROUP` | No | `1` | Default subgroup: `1` or `2`. |
| `NOTIFY_TIME` | No | `07:30` | Morning notification time in `HH:MM` format. |
| `TIMEZONE` | No | `Asia/Yekaterinburg` | Timezone used for current day selection and notifications. |

Generate `CREDENTIALS_SECRET` with:

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

Logs:

```bash
docker compose logs -f ktk-schedule
```

Stop:

```bash
docker compose down
```

### Subgroups

Workspace exposes lesson subgroup information through the `Subgroup` field.

| API Value | Bot Meaning | Behavior |
| --- | --- | --- |
| `left` | subgroup 1 | Visible to subgroup one users. |
| `right` | subgroup 2 | Visible to subgroup two users. |
| `middle` | common lesson | Visible to everyone. |

By default, the bot uses `DEFAULT_SUBGROUP`. If workspace returns the user's subgroup after sign-in, the bot will try to detect it automatically.

### Security

User passwords are not stored in plaintext. Before writing to SQLite, the password is encrypted with AES-GCM using a key derived from `CREDENTIALS_SECRET`.

Important: if `CREDENTIALS_SECRET` is changed or lost, previously stored passwords cannot be decrypted. Users will need to run `/login` again.

### Debug Mode

To inspect the schedule response shape, temporarily enable:

```env
KTK_DEBUG_SCHEDULE=true
```

After `/schedule`, logs will include a short summary:

```text
schedule debug: days=6 subjects=12
schedule debug item day=0: { ... }
```

Do not keep debug mode enabled unless you need diagnostics.

### Tests And Checks

```bash
go test -count=1 ./...
go vet ./...
go build -o NUL ./cmd/bot
```

On Linux/macOS, use `/dev/null` instead of `NUL`:

```bash
go build -o /dev/null ./cmd/bot
```

### Project Structure

| Path | Purpose |
| --- | --- |
| `cmd/bot` | Application entry point. |
| `internal/app` | Telegram handlers, sessions, notifications. |
| `internal/config` | `.env` loading and configuration validation. |
| `internal/ktk` | Workspace client, API endpoint discovery, schedule formatting. |
| `internal/storage` | SQLite persistence and migrations. |
| `internal/credentials` | Encryption and decryption for stored passwords. |
| `internal/tg` | Telegram UI helpers and inline keyboards. |

### ⚠️ Disclaimer

This project uses a third-party service API. The author is not affiliated with KTK and is not responsible for workspace API changes, service downtime, or response format changes.

## Cat in the readme 🐈

<p align="center">
    <img src="https://cataas.com/cat" align="center" width="480" />
</p>
