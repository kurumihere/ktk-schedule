# ktk-schedule

Telegram бот для просмотра расписания КТК.

## Возможности

- Авторизация через workspace
- Просмотр расписания по дням
- Утренние уведомления
- Шифрование сохранённых паролей через `CREDENTIALS_SECRET`

## Настройка

Создай `.env` по примеру `.env.example` и обязательно задай `CREDENTIALS_SECRET` длиной минимум 32 символа.
```bash
$ openssl rand -base64 24
```

[!IMPORTANT]
Если поменять или потерять `CREDENTIALS_SECRET`, сохранённые пароли нельзя будет расшифровать. Пользователям потребуется заново выполнить `/login`.

[!WARNING]
## ⚠️ Disclaimer
Проект использует публичный API стороннего сервиса. Автор не связан с КТК и не несёт ответственности за изменения API.

## Cat in the readme 🐈

<p align="center">
    <img src="https://cataas.com/cat" align="center" width="480" />
</p>
