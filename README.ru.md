# ktk-schedule [![Build Status](https://github.com/kurumihere/ktk-schedule/actions/workflows/ci.yml/badge.svg?branch=master&event=push)](https://github.com/kurumihere/ktk-schedule/actions/workflows/ci.yml) [![CodeFactor](https://www.codefactor.io/repository/github/kurumihere/ktk-schedule/badge/master)](https://www.codefactor.io/repository/github/kurumihere/ktk-schedule/overview/master)

[English version](./README.md)

Это телеграм-бот для студентов и преподавателей [КТК](https://ktk-45.ru). Он использует API сайта колледжа и получает актуальные данные из [workspace](https://workspace.ktk-45.ru)

Бот написан на [Go](https://go.dev) с использованием [go-telegram/bot](https://github.com/go-telegram/bot), а данные пользователей и расписание хранятся в [SQLite](https://sqlite.org)

## Возможности

- Расписание на текущую неделю или выбранную дату
- Удобная навигация между днями и неделями
- Авторизация студентов и преподавателей через данные для входа в workspace
- Выбор группы, подгруппы и просмотр расписания других групп
- Задания, вложения и файлы по дням
- Утренние уведомления с расписанием
- Сохранённое расписание при временной недоступности workspace

## Лицензия

Проект распространяется по лицензии [BSD 3-Clause](./LICENSE). Его можно использовать, копировать, изменять и распространять в исходном или бинарном виде, в том числе в коммерческих проектах, при условии сохранения уведомления об авторских правах и текста лицензии

Имена правообладателя и участников нельзя использовать для продвижения производных продуктов без разрешения. Программное обеспечение предоставляется как есть, без гарантий и ответственности
