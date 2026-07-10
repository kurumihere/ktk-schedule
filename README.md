# ktk-schedule [![build status](https://github.com/kurumihere/ktk-schedule/actions/workflows/ci.yml/badge.svg?branch=master&event=push)](https://github.com/kurumihere/ktk-schedule/actions/workflows/ci.yml) [![codefactor](https://www.codefactor.io/repository/github/kurumihere/ktk-schedule/badge/master)](https://www.codefactor.io/repository/github/kurumihere/ktk-schedule/overview/master)

[russian version](./README.ru.md)

this is a telegram-bot for students and teachers at [KTK](https://ktk-45.ru). it uses the college API and retrieves up-to-date data from [workspace](https://workspace.ktk-45.ru)

the bot is written in [Go](https://go.dev) using [go-telegram/bot](https://github.com/go-telegram/bot), while user data and schedules are stored in [SQLite](https://sqlite.org)

## features

- schedule for the current week or a selected date
- convenient navigation between days and weeks
- student and teacher authentication through workspace
- group and subgroup selection with access to other groups
- assignments, attachments, and files for each day
- morning schedule notifications
- cached schedules when workspace is temporarily unavailable

## license

the project is distributed under the [BSD 3-Clause](./LICENSE) license. it may be used, copied, modified, and redistributed in source or binary form, including for commercial purposes, as long as the copyright notice and license terms are preserved

the names of the copyright holder and contributors may not be used to endorse derived products without permission. the software is provided as is, without warranties or liability
