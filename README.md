# ktk-schedule [![Build Status](https://github.com/kurumihere/ktk-schedule/actions/workflows/ci.yml/badge.svg?branch=master&event=push)](https://github.com/kurumihere/ktk-schedule/actions/workflows/ci.yml) [![CodeFactor](https://www.codefactor.io/repository/github/kurumihere/ktk-schedule/badge/master)](https://www.codefactor.io/repository/github/kurumihere/ktk-schedule/overview/master)

[Russian version](./README.ru.md)

This is a telegram-bot for students and teachers at [KTK](https://ktk-45.ru). It uses the college website's API and retrieves up-to-date data from [workspace](https://workspace.ktk-45.ru)

The bot is written in [Go](https://go.dev) using [go-telegram/bot](https://github.com/go-telegram/bot), and user data and the schedule are stored in [SQLite](https://sqlite.org)

## Features

- Schedule for the current week or a selected date
- Convenient navigation between days and weeks
- Student and teacher authentication using workspace login credentials
- Group and subgroup selection and viewing schedules of other groups
- Assignments, attachments, and files by day
- Morning schedule notifications
- Saved schedule when workspace is temporarily unavailable

## License

The project is distributed under the [BSD 3-Clause](./LICENSE) license. It may be used, copied, modified, and redistributed in source or binary form, including for commercial purposes, as long as the copyright notice and license terms are preserved

The names of the copyright holder and contributors may not be used to endorse derived products without permission. The software is provided as is, without warranties or liability
