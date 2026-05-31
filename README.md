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
  Telegram bot for KTK schedule: workspace login, schedule navigation,
  subgroups, grades, files, and morning notifications.
</p>

<p align="center">
  <a href="#русский">Русский</a> |
  <a href="#english">English</a>
</p>

## Русский

`ktk-schedule` - это Telegram-бот для расписания КТК. Он работает через workspace: пользователь входит по своему логину и паролю, а бот показывает расписание, оценки, отметки посещаемости, файлы из домашки и отправленные работы.

Я делал его не как “витрину”, а как рабочий бот на каждый день: открыть текущую неделю, быстро переключиться на нужный день, скачать файл от преподавателя, включить утреннее расписание и не думать о том, какой endpoint сейчас снова поменялся на сайте.

### Что умеет

- Авторизует студентов и преподавателей через workspace.
- Сам находит актуальные API endpoint-ы из workspace assets.
- Показывает расписание на неделю или конкретную дату: `/schedule`, `/schedule 01.09`, `/schedule 2026-09-01`.
- Поддерживает группы, 1-ю и 2-ю подгруппу, а также режим показа обеих подгрупп сразу.
- Показывает время пары, длительность, текущий статус и сколько осталось до конца.
- Выводит оценки `2-5`, модификаторы `+/-`, отметки `Н/О/Б` и причины отсутствий, если они есть.
- Показывает и скачивает прикреплённые файлы: материалы из домашки и загруженную пользователем работу.
- Умеет отправлять утренние уведомления с расписанием.
- Даёт владельцу рассылку через `/announce` и статистику через `/stats`.
- Хранит пароли зашифрованными в SQLite и удаляет сообщение с `/login` после обработки.
- Использует cache, retry с backoff, rate limit, circuit breaker и HTTP `/health`.

### Команды

| Команда | Что делает |
| --- | --- |
| `/start` | Показывает список команд |
| `/my_id` | Показывает твой Telegram ID |
| `/login логин пароль` | Авторизует в workspace |
| `/schedule [дата]` | Показывает расписание на неделю или дату |
| `/group 269` | Меняет группу |
| `/subgroup 1` / `/subgroup 2` | Меняет подгруппу |
| `/subgroups_on` / `/subgroups_off` | Показывает обе подгруппы или только выбранную |
| `/notify_on` / `/notify_off` | Включает или выключает утреннее расписание |
| `/announce текст` | Рассылает объявление от владельца |
| `reply /announce` | Рассылает сообщение, на которое был дан ответ |
| `/stats` | Показывает статистику бота владельцу |

После `/schedule` появляется inline-клавиатура. Через неё можно листать дни, перейти к сегодняшнему дню, переключать недели, открыть выбор недели и скачать все файлы выбранного дня кнопкой `📎 Скачать файлы (N)`.

### Быстрый старт

```bash
cp .env.example .env
# Заполни BOT_TOKEN и CREDENTIALS_SECRET
go run ./cmd/bot
```

Секрет для шифрования можно сгенерировать так:

```bash
openssl rand -base64 32
```

### Docker

```bash
docker compose up --build -d
docker compose logs -f ktk-schedule
```

В Docker бот использует SQLite и отдаёт `/health`, чтобы контейнер можно было проверять healthcheck-ом.

### Конфигурация

Все переменные описаны в `.env.example`. Для запуска обычно достаточно этих:

| Переменная | Зачем нужна |
| --- | --- |
| `BOT_TOKEN` | Токен Telegram-бота от `@BotFather` |
| `CREDENTIALS_SECRET` | Секрет для AES-GCM, минимум 32 символа |
| `OWNER_TELEGRAM_ID` | Владелец команд `/announce` и `/stats` |
| `KTK_BASE_URL` | Базовый URL workspace |
| `DATABASE_PATH` | Путь к SQLite базе |
| `NOTIFY_TIME` / `TIMEZONE` | Время и часовой пояс уведомлений |

### Разработка

Нужны Go 1.26+, `just` и `golangci-lint`. Для hot reload используется `air`.

```bash
just setup       # настроить pre-commit hook
just setup-air   # установить air
just setup-lint  # установить golangci-lint
just dev         # запустить hot reload
just test        # go test -count=1 ./...
just lint        # golangci-lint run
just check       # env-check + fmt + vet + test + build
just build       # собрать бинарник ktk-schedule
```

### Как устроен проект

| Путь | Что внутри |
| --- | --- |
| `cmd/bot` | Точка входа |
| `internal/app` | Handlers, sessions, notifications, cache, rate limit |
| `internal/config` | Загрузка и проверка `.env` |
| `internal/ktk` | Workspace client, autodiscovery, parsing и formatting расписания |
| `internal/storage` | SQLite-хранилище и миграции |
| `internal/credentials` | Шифрование паролей |
| `internal/tg` | Telegram inline-клавиатуры |
| `.gitea/workflows` | CI/CD |
| `Justfile` | Команды для разработки |

### Пример сообщения

```text
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

3 пара [70 мин] — ОП.02 ДМЭМЛ
⏰ 11:00-12:10
🔬 Практическое занятие
👤 Сапожникова Елена Владимировна
🏫 Кабинет: 41
```

### Важно

Пароли пользователей шифруются перед записью в базу. Если поменять `CREDENTIALS_SECRET`, всем пользователям придётся войти заново.

Проект использует сторонний workspace API. Автор не связан с КТК и не отвечает за изменения API, недоступность сервиса или неожиданные изменения формата данных.

---

## English

`ktk-schedule` is a Telegram bot for KTK schedules. It works through workspace: a user signs in with their login and password, and the bot shows schedule, grades, attendance marks, homework files, and submitted work.

I built it as a daily-use bot, not a demo page: open the current week, jump to the right day, download a teacher's file, enable morning schedule notifications, and avoid caring about which workspace endpoint changed again.

### What It Can Do

- Signs students and teachers in through workspace.
- Discovers current API endpoints from workspace assets.
- Shows schedule for a week or a specific date: `/schedule`, `/schedule 01.09`, `/schedule 2026-09-01`.
- Supports groups, 1st and 2nd subgroups, and a mode that shows both subgroups at once.
- Shows lesson time, duration, current status, and time left.
- Shows grades `2-5`, `+/-` modifiers, `Н/О/Б` attendance marks, and absence reasons when available.
- Shows and downloads attached files: homework materials and the user's submitted work.
- Sends morning schedule notifications.
- Gives the owner broadcasts through `/announce` and statistics through `/stats`.
- Stores passwords encrypted in SQLite and deletes the `/login` message after processing.
- Uses cache, retry with backoff, rate limit, circuit breaker, and HTTP `/health`.

### Commands

| Command | What it does |
| --- | --- |
| `/start` | Shows the command list |
| `/my_id` | Shows your Telegram ID |
| `/login login password` | Signs in to workspace |
| `/schedule [date]` | Shows schedule for a week or date |
| `/group 269` | Changes group |
| `/subgroup 1` / `/subgroup 2` | Changes subgroup |
| `/subgroups_on` / `/subgroups_off` | Shows both subgroups or only the selected one |
| `/notify_on` / `/notify_off` | Enables or disables morning schedule messages |
| `/announce text` | Sends an owner announcement |
| `reply /announce` | Broadcasts the replied message |
| `/stats` | Shows bot statistics to the owner |

After `/schedule`, the bot shows an inline keyboard. It lets you switch days, return to today, switch weeks, open week selection, and download all files for the selected day with `📎 Скачать файлы (N)`.

### Quick Start

```bash
cp .env.example .env
# Fill BOT_TOKEN and CREDENTIALS_SECRET
go run ./cmd/bot
```

Generate an encryption secret:

```bash
openssl rand -base64 32
```

### Docker

```bash
docker compose up --build -d
docker compose logs -f ktk-schedule
```

In Docker, the bot uses SQLite and exposes `/health` so the container can be checked by a healthcheck.

### Configuration

All variables are documented in `.env.example`. These are usually enough to start:

| Variable | Why it is needed |
| --- | --- |
| `BOT_TOKEN` | Telegram bot token from `@BotFather` |
| `CREDENTIALS_SECRET` | AES-GCM secret, at least 32 characters |
| `OWNER_TELEGRAM_ID` | Owner of `/announce` and `/stats` |
| `KTK_BASE_URL` | Workspace base URL |
| `DATABASE_PATH` | SQLite database path |
| `NOTIFY_TIME` / `TIMEZONE` | Notification time and timezone |

### Development

You need Go 1.26+, `just`, and `golangci-lint`. Hot reload uses `air`.

```bash
just setup       # configure pre-commit hook
just setup-air   # install air
just setup-lint  # install golangci-lint
just dev         # run hot reload
just test        # go test -count=1 ./...
just lint        # golangci-lint run
just check       # env-check + fmt + vet + test + build
just build       # build the ktk-schedule binary
```

### Project Layout

| Path | What is inside |
| --- | --- |
| `cmd/bot` | Entry point |
| `internal/app` | Handlers, sessions, notifications, cache, rate limit |
| `internal/config` | `.env` loading and validation |
| `internal/ktk` | Workspace client, autodiscovery, schedule parsing and formatting |
| `internal/storage` | SQLite storage and migrations |
| `internal/credentials` | Password encryption |
| `internal/tg` | Telegram inline keyboards |
| `.gitea/workflows` | CI/CD |
| `Justfile` | Development commands |

### Message Example

```text
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

3 пара [70 мин] — ОП.02 ДМЭМЛ
⏰ 11:00-12:10
🔬 Практическое занятие
👤 Сапожникова Елена Владимировна
🏫 Кабинет: 41
```

### Notes

User passwords are encrypted before being written to the database. If `CREDENTIALS_SECRET` changes, every user has to sign in again.

This project uses a third-party workspace API. The author is not affiliated with KTK and is not responsible for API changes, downtime, or unexpected data format changes.
