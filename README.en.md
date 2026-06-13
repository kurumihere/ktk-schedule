# ktk-schedule

[Русская версия](./README.md)

Telegram bot for the KTK workspace schedule. It signs users in, stores data in SQLite, shows schedules in Telegram, and sends morning notifications.

## Features

- weekly, daily, and date-based schedules;
- student and teacher sign-in;
- groups, subgroups, and another-group view;
- homework attachments and day files;
- morning notifications;
- encrypted password storage in SQLite;
- Docker healthcheck for deployment.

## Run

```bash
cp .env.example .env
```

Minimum required values:

```dotenv
BOT_TOKEN=123456:token-from-botfather
CREDENTIALS_SECRET=random-string-at-least-32-characters
```

Generate a secret:

```bash
openssl rand -base64 32
```

Locally:

```bash
go run ./cmd/bot
```

With Docker:

```bash
docker compose up --build -d
docker compose logs -f
```

## Bot Commands

| Command | Purpose |
|---|---|
| `/start` | show commands |
| `/my_id` | show Telegram ID |
| `/login username password` | sign in to workspace |
| `/schedule` | open schedule |
| `/schedule 01.09` | schedule for a date |
| `/notify_on` / `/notify_off` | enable or disable notifications |
| `/announce text` | broadcast owner announcement |
| `/stats` | owner statistics |

`/login` deletes the user message after processing so credentials do not stay in chat history.

## Configuration

All variables are listed in [.env.example](./.env.example). Main ones:

| Variable | Purpose |
|---|---|
| `BOT_TOKEN` | Telegram bot token |
| `CREDENTIALS_SECRET` | password encryption secret |
| `OWNER_TELEGRAM_ID` | owner for `/announce` and `/stats` |
| `DATABASE_PATH` | SQLite path |
| `NOTIFY_TIME` | morning notification time |
| `TIMEZONE` | time zone |
| `HEALTH_ADDR` | `/health` address |

## Development

Go 1.26.4+ is required. `just` is optional and only wraps regular commands:

```bash
just test     # go test ./...
just build    # go build ./cmd/bot
just check    # fmt, vet, test, build
just docker   # docker compose up --build -d
just logs     # docker compose logs -f
just down     # docker compose down
```

Without `just`:

```bash
go fmt ./...
go vet ./...
go test ./...
go build -trimpath -ldflags="-s -w" -o ktk-schedule ./cmd/bot
```

## Deploy

GitHub Actions are split into two small workflows:

- `Continuous Integration`: tests and binary build for pushes and pull requests.
- `Deploy`: after successful CI on `master`, builds the Docker image, pushes it to GHCR, and deploys `production` through GitHub Deployments.

Before deployment, the workflow creates a SQLite backup. If the new container does not become healthy, it tries to roll back to the previous image.

## Layout

| Path | Purpose |
|---|---|
| `cmd/bot` | entry point |
| `internal/app` | Telegram handlers, notifications, health |
| `internal/ktk` | workspace client and schedule formatting |
| `internal/storage` | SQLite |
| `internal/config` | `.env` loading |
| `.github/workflows` | CI and deploy |

## License

[BSD 3-Clause](./LICENSE)
