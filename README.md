# ktk-schedule

[English version](./README.en.md)

Telegram-бот для расписания workspace КТК. Он авторизует пользователя, хранит данные в SQLite, показывает расписание в Telegram и отправляет утренние уведомления.

## Возможности

- расписание на неделю, день или конкретную дату;
- вход студента или преподавателя;
- группы, подгруппы и просмотр другой группы;
- вложения к заданиям и файлы по дням;
- утренние уведомления;
- SQLite с шифрованием паролей;
- healthcheck для Docker и деплоя.

## Запуск

```bash
cp .env.example .env
```

Минимально нужно заполнить:

```dotenv
BOT_TOKEN=123456:token-from-botfather
CREDENTIALS_SECRET=random-string-at-least-32-characters
```

Сгенерировать секрет:

```bash
openssl rand -base64 32
```

Локально:

```bash
go run ./cmd/bot
```

В Docker:

```bash
docker compose up --build -d
docker compose logs -f
```

## Команды бота

| Команда | Что делает |
|---|---|
| `/start` | показать команды |
| `/my_id` | показать Telegram ID |
| `/login логин пароль` | войти в workspace |
| `/schedule` | открыть расписание |
| `/schedule 01.09` | расписание на дату |
| `/notify_on` / `/notify_off` | включить или выключить уведомления |
| `/announce текст` | отправить объявление всем пользователям |
| `/stats` | статистика для владельца |

`/login` удаляет сообщение пользователя после обработки, чтобы логин и пароль не оставались в чате.

## Конфигурация

Все переменные лежат в [.env.example](./.env.example). Основные:

| Переменная | Назначение |
|---|---|
| `BOT_TOKEN` | токен Telegram-бота |
| `CREDENTIALS_SECRET` | секрет для шифрования паролей |
| `OWNER_TELEGRAM_ID` | владелец команд `/announce` и `/stats` |
| `DATABASE_PATH` | путь к SQLite |
| `NOTIFY_TIME` | время утренней рассылки |
| `TIMEZONE` | часовой пояс |
| `HEALTH_ADDR` | адрес `/health` |

## Разработка

Нужен Go 1.26.4+. `just` необязателен, но даёт короткие команды:

```bash
just test     # go test ./...
just build    # go build ./cmd/bot
just check    # fmt, vet, test, build
just docker   # docker compose up --build -d
just logs     # docker compose logs -f
just down     # docker compose down
```

Без `just` можно запускать те же команды напрямую:

```bash
go fmt ./...
go vet ./...
go test ./...
go build -trimpath -ldflags="-s -w" -o ktk-schedule ./cmd/bot
```

## Deploy

GitHub Actions разделены на два простых workflow:

- `Continuous Integration`: тесты и сборка бинарника для push/PR.
- `Deploy`: после успешного CI на `master` собирает Docker image, публикует его в GHCR и выкатывает `production` через GitHub Deployments.

Перед выкладкой создаётся backup SQLite. Если новый контейнер не становится healthy, workflow пробует вернуть предыдущий image.

## Структура

| Путь | Назначение |
|---|---|
| `cmd/bot` | точка входа |
| `internal/app` | Telegram-хендлеры, уведомления, health |
| `internal/ktk` | клиент workspace и форматирование расписания |
| `internal/storage` | SQLite |
| `internal/config` | загрузка `.env` |
| `.github/workflows` | CI и deploy |

## Лицензия

[BSD 3-Clause](./LICENSE)
