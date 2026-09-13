# FEATURES - ktk-schedule features

Non-exhaustive list of what the bot can do. Keep this updated as commands/buttons are added/removed.

Most views are switchable via the inline buttons under the schedule, with sensible opinionated defaults. Personal settings (subgroup, notifications) persist between restarts.

👑 - owner only (`OWNER_TELEGRAM_ID`)
🎓 - students only (hidden for teacher accounts)

---

## English

### Auth & privacy

- `/login login password` - sign in to Workspace. The password message is deleted; if deletion fails, login is cancelled and the password goes nowhere
- `/logout` - sign out: deletes saved credentials, schedule cache, navigation state, ends the bot session, disables notifications
- `/my_id` - show your Telegram ID (needed for `OWNER_TELEGRAM_ID`)
- login rate limit: 5 attempts / 60s, then blocked for 300s
- personal commands and buttons work only in a private chat with the bot

### Schedule view

- `/schedule` - today; `/schedule 01.09`, `/schedule 01.09.2026`, `/schedule 2026-09-01` - a date (the day opens inside its own week)
- one day per message, lessons in order, empty lessons marked as empty
- per lesson: start/end time and duration, lesson type, teacher (🎓), classroom, group (when set)
- today is labelled, the current/next lesson shows progress ("in progress", "left", "starts in")
- week header shows the study-week number and date range
- non-school day vs empty week are told apart
- 🎓 grades and marks: appraisal, `+`/`-`, absences with captions
- 🎓 homework: task text, webinar, file count, file names (up to 20), submitted work ("my work")

### Navigation buttons

- week left/right - previous/next week
- week label - week picker: 5 weeks around the current one, paging by 5, "current week" return
- day left/right - previous/next school day; `Today` - jump back; `Refresh` - force re-fetch from the site
- day buttons (`Mon 01.09`, ...) - jump to a day, current one marked ✅
- 🎓 `1st subgroup / 2nd subgroup / Both` - subgroup filter, saved for your own schedule
- `Group schedule / Another group` - view another group: type the number as a message (e.g. `269`), `Back` exits input, `My group` returns
- `Download files (N)` - appears when the day has files; sends the list plus each file as a separate message

### Subgroups

- your subgroup comes from Workspace at `/login`, switchable via buttons
- canon `left` = 1st, `right` = 2nd; `Both` shows both with `[1]`/`[2]` tags
- in another group's view the subgroup filter applies to that view only

### Groups & teachers

- another group is fetched separately from the personal one: no personal grades/homework leak into it, header gains a "Group N schedule" line
- teacher accounts (`IsStudent=false`) get the full picture instead of grades/homework/subgroup buttons

### Homework & files

- file names resolved from Workspace, icon by type (pdf, images, word, excel, slides, archives)
- download: streamed, 50 MiB per file, up to 64 files per day, links only from the Workspace origin
- offline: honest "files unavailable without connection" instead of a broken result

### Morning notifications

- `/notify_on` - enable, `/notify_off` - disable; time = `NOTIFY_TIME` (default `07:30` in `TIMEZONE`)
- school days only, starts with "Good morning. Today's schedule:"
- no repeat delivery on the same day

### Owner commands

- 👑 `/stats` - users, notification subscriptions, active sessions, uptime, timezone
- 👑 `/announce text` or reply-`/announce` - broadcast to all authorized users, ends with a "Delivered / Errors" report; disabled when `OWNER_TELEGRAM_ID=0`

### Reliability

- 5-minute cache; when the site is down, the saved schedule is shown with a "site unavailable, showing saved schedule" note
- circuit breaker (5 failures - 30s pause), max 12 parallel requests and 3 parallel downloads
- long messages truncated to the Telegram 4096 limit (counted in UTF-16, unicode never split)
- request failure answers "could not fetch the schedule, try later", never an internal dump

---

## Русский

### Вход и приватность

- `/login логин пароль` - вход в Workspace. Сообщение с паролем удаляется; если удаление не удалось, вход отменяется и пароль никуда не отправляется
- `/logout` - выход: удаляет сохраненные логин/пароль, кеш расписаний, состояние кнопок, завершает сессию в боте, отключает уведомления
- `/my_id` - показать свой Telegram ID (нужен для `OWNER_TELEGRAM_ID`)
- лимит входа: 5 попыток / 60 с, затем блок на 300 с
- личные команды и кнопки работают только в личном чате с ботом

### Просмотр расписания

- `/schedule` - сегодня; `/schedule 01.09`, `/schedule 01.09.2026`, `/schedule 2026-09-01` - дата (день открывается внутри своей недели)
- один день на сообщение, пары по порядку, пустые пары помечены
- по каждой паре: время начала/конца и длительность, тип занятия, преподаватель (🎓), кабинет, группа (если задана)
- сегодняшний день помечен, у текущей/ближайшей пары показан прогресс ("идет", "осталось", "начнется через")
- заголовок недели: номер учебной недели и диапазон дат
- неучебный день и пустая неделя различаются
- 🎓 оценки и отметки: балл, `+`/`-`, пропуски с расшифровкой
- 🎓 домашнее задание: текст, вебинар, число файлов, имена файлов (до 20), сданная работа ("моя работа")

### Кнопки навигации

- неделя влево/вправо - предыдущая/следующая неделя
- подпись недели - выбор недели: 5 недель вокруг текущей, листание по 5, возврат "Текущая неделя"
- день влево/вправо - предыдущий/следующий учебный день; `Сегодня` - вернуться; `Обновить` - перезапросить день с сайта
- кнопки дней (`Пн 01.09`, ...) - переход к дню, текущий отмечен ✅
- 🎓 `1 подгруппа / 2 подгруппа / Обе` - фильтр подгруппы, сохраняется для своего расписания
- `Расписание группы / Другая группа` - просмотр чужой группы: номер сообщением (например `269`), `Назад` - выйти из ввода, `Своя группа` - вернуться
- `Скачать файлы (N)` - появляется при файлах у дня; присылает список и каждый файл отдельным сообщением

### Подгруппы

- своя подгруппа берется из Workspace при `/login`, меняется кнопками
- канон `left` = 1-я, `right` = 2-я; `Обе` показывает обе с метками `[1]`/`[2]`
- в чужой группе фильтр подгруппы действует только на этот просмотр

### Группы и преподаватели

- чужая группа запрашивается отдельно от личной: оценки и ДЗ туда не попадают, сверху добавляется строка "Расписание группы N"
- аккаунт преподавателя (`IsStudent=false`) видит общую картину вместо оценок/ДЗ и кнопок подгрупп

### Домашка и файлы

- имена файлов из Workspace, иконка по типу (pdf, картинки, word, excel, презентации, архивы)
- скачивание: стрим, 50 МБ на файл, до 64 файлов на день, ссылки только с адреса Workspace
- без связи с сайтом - честное "Файлы недоступны без соединения" вместо битого результата

### Утренние уведомления

- `/notify_on` - включить, `/notify_off` - выключить; время - `NOTIFY_TIME` (по умолчанию `07:30` в `TIMEZONE`)
- только в учебные дни, текст начинается с "Доброе утро. Расписание на сегодня:"
- повтор в тот же день не отправляется

### Команды владельца

- 👑 `/stats` - пользователи, подписки на уведомления, активные сессии, аптайм, таймзона
- 👑 `/announce текст` или реплай `/announce` - рассылка всем авторизованным, в конце отчет "Доставлено / Ошибок"; выключена при `OWNER_TELEGRAM_ID=0`

### Надежность

- кеш на 5 минут; при недоступности сайта показывается сохраненное расписание с пометкой "Сайт расписания недоступен, показываю сохраненное расписание."
- circuit breaker (5 ошибок - пауза 30 с), не более 12 параллельных запросов и 3 параллельных скачиваний
- длинные сообщения обрезаются до лимита Telegram 4096 (считается в UTF-16, юникод не рвется)
- ошибка запроса отвечает "Не удалось получить расписание. Попробуй позже.", а не внутренним дампом
