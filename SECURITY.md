# Security

English version first, русская версия ниже. Both halves describe the same guarantees - keep them in sync.

## English

Authorization and personal schedule viewing work only in a private chat with the bot. After `/login login password` the bot deletes the message containing the credentials. If Telegram does not confirm the deletion, login is cancelled: the bot neither sends this data to Workspace nor saves it. Login attempts are rate-limited.

The bot stores credentials so you do not have to type them for every schedule request. The password is stored encrypted. Saved schedules, including grades and homework, and the button navigation state are also encrypted. Telegram ID, login and group settings are stored unencrypted.

The personal cache is bound to the user. When viewing another group, the bot requests its schedule separately from personal grades and homework. Buttons under messages verify the message owner: another user cannot control your schedule through them.

The bot connects to Workspace over HTTPS. Files are downloaded only from the configured Workspace address; links and redirects to foreign servers are blocked. Downloadable file size is limited.

`/logout` in a private chat deletes the saved credentials, the schedule cache and the navigation state, ends the active session in the bot and disables notifications. After logout you need to authorize again to access the schedule. Previously sent Telegram messages and sessions on the college website remain.

Encrypting stored data does not protect against access to a running bot together with its encryption key. Backups may contain data saved before logout, and old copies may contain an unencrypted cache.

## Русский

Авторизация и просмотр персонального расписания доступны только в личном чате с ботом. После команды `/login логин пароль` бот удаляет сообщение с данными входа. Если Telegram не подтверждает удаление, вход отменяется: бот не отправляет эти данные в Workspace и не сохраняет их. Число попыток входа ограничено.

Бот сохраняет данные входа, чтобы пользователю не приходилось вводить их при каждом запросе расписания. Пароль хранится в зашифрованном виде. Сохранённые расписания, включая оценки и домашние задания, и состояние кнопок навигации также шифруются. Telegram ID, логин и настройки группы хранятся без шифрования.

Персональный кеш привязан к пользователю. При просмотре другой группы бот запрашивает её расписание отдельно от персональных оценок и домашних заданий. Кнопки под сообщениями проверяют пользователя, которому принадлежит сообщение: другой пользователь не может управлять его расписанием через эти кнопки.

Бот подключается к Workspace по HTTPS. Файлы скачиваются только с настроенного адреса Workspace; ссылки и перенаправления на посторонние серверы блокируются. Размер скачиваемого файла ограничен.

Команда `/logout` в личном чате удаляет сохранённые данные входа, кеш расписаний и состояние навигации, завершает активную сессию в боте и отключает уведомления. После выхода для доступа к расписанию нужно авторизоваться заново. Ранее отправленные сообщения в Telegram и сессии на сайте колледжа остаются.

Шифрование сохранённых данных не защищает от доступа к работающему боту вместе с его ключом шифрования. Резервные копии могут содержать данные, сохранённые до выхода из аккаунта, а старые копии — незашифрованный кеш.
