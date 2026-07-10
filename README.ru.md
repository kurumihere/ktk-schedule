# ktk-schedule [![build status](https://github.com/kurumihere/ktk-schedule/actions/workflows/ci.yml/badge.svg?branch=master&event=push)](https://github.com/kurumihere/ktk-schedule/actions/workflows/ci.yml) [![codefactor](https://www.codefactor.io/repository/github/kurumihere/ktk-schedule/badge/master)](https://www.codefactor.io/repository/github/kurumihere/ktk-schedule/overview/master)

[english version](./README.md)

это телеграм-бот для студентов и преподавателей [КТК](https://ktk-45.ru). он использует API колледжа и получает актуальные данные из [workspace](https://workspace.ktk-45.ru)

бот написан на [Go](https://go.dev) с использованием [go-telegram/bot](https://github.com/go-telegram/bot), а данные пользователей и расписание хранятся в [SQLite](https://sqlite.org)

## возможности

- расписание на текущую неделю или выбранную дату
- удобная навигация между днями и неделями
- авторизация студентов и преподавателей через workspace
- выбор группы, подгруппы и просмотр расписания других групп
- задания, вложения и файлы по дням
- утренние уведомления с расписанием
- сохранённое расписание при временной недоступности workspace

## лицензия

проект распространяется по лицензии [BSD 3-Clause](./LICENSE). его можно использовать, копировать, изменять и распространять в исходном или бинарном виде, в том числе в коммерческих проектах, при условии сохранения уведомления об авторских правах и текста лицензии

имена правообладателя и участников нельзя использовать для продвижения производных продуктов без разрешения. программное обеспечение предоставляется как есть, без гарантий и ответственности
