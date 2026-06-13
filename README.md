# ktk-schedule

[![Build status](https://github.com/kurumihere/ktk-schedule/actions/workflows/ci.yml/badge.svg?branch=master&event=push)](https://github.com/kurumihere/ktk-schedule/actions/workflows/ci.yml)


[Russian version](./README.ru.md)

Telegram bot for the [KTK](ktk-45.ru) schedule.

## Features

- Schedule for a week, day, or specific date
- Student or teacher sign-in
- Groups, subgroups, and viewing another group
- Assignment attachments and files by day
- Morning notifications

## Configuration

All variables are listed in [.env.example](./.env.example). Main ones:

| Variable | Purpose |
|---|---|
| `BOT_TOKEN` | Telegram bot token |
| `CREDENTIALS_SECRET` | password encryption secret |
| `OWNER_TELEGRAM_ID` | owner of the `/announce` and `/stats` commands |
| `DATABASE_PATH` | SQLite path |
| `NOTIFY_TIME` | morning notification time |
| `TIMEZONE` | time zone |

## License

[BSD 3-Clause](./LICENSE)
