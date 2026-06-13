# ktk-schedule

[![Build status](https://github.com/kurumihere/ktk-schedule/actions/workflows/ci.yml/badge.svg?branch=master&event=push)](https://github.com/kurumihere/ktk-schedule/actions/workflows/ci.yml)


[English version](./README.md)

Telegram-бот для расписания [КТК](ktk-45.ru).

## Возможности

- Расписание на неделю, день или конкретную дату
- Вход студента или преподавателя
- Группы, подгруппы и просмотр другой группы
- Вложения к заданиям и файлы по дням
- Утренние уведомления

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

## Лицензия

[BSD 3-Clause](./LICENSE)
