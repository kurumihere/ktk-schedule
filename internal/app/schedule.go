package app

import (
	"context"
	"encoding/json"
	"errors"
	"log/slog"
	"time"

	telegram "github.com/go-telegram/bot"

	"ktk-schedule/internal/ktk"
	"ktk-schedule/internal/storage"
)

func (a *App) ensureSession(ctx context.Context, user *storage.User) (*Session, error) {
	if session := a.getSession(user.TelegramID); session != nil && session.Client != nil {
		a.syncTeacherHash(user, session.Client.TeacherHash())

		var halls ktk.LectureHallMap
		var callPresets ktk.CallPresetMap
		var absenceMarks []ktk.AbsenceMark
		var pairTypes ktk.PairTypeMap

		if session.Halls == nil {
			halls, _ = a.loadLectureHalls(ctx, session.Client, user.GroupID)
			if halls == nil {
				halls = make(ktk.LectureHallMap)
			}
		}
		if session.CallPresets == nil {
			callPresets = a.loadCallPresets(ctx, session.Client)
		}
		if session.AbsenceMarks == nil {
			absenceMarks = a.loadAbsenceMarks(ctx, session.Client)
		}
		if session.PairTypes == nil {
			pairTypes = a.loadPairTypes(ctx, session.Client)
		}
		if user.PasswordLegacy {
			a.migrateLegacyPassword(user)
		}

		a.modifySession(user.TelegramID, func(s *Session) {
			s.Subgroup = user.Subgroup
			s.ShowAllSubgroups = user.ShowAllSubgroups
			s.TeacherHash = user.TeacherHash
			if s.Halls == nil && halls != nil {
				s.Halls = halls
			}
			if s.CallPresets == nil && callPresets != nil {
				s.CallPresets = callPresets
			}
			if s.AbsenceMarks == nil && absenceMarks != nil {
				s.AbsenceMarks = absenceMarks
			}
			if s.PairTypes == nil && pairTypes != nil {
				s.PairTypes = pairTypes
			}
		})

		return a.getSession(user.TelegramID), nil
	}

	client, err := a.authClient(ctx, user.Login, user.Password, user.GroupID)
	if err != nil {
		return nil, err
	}
	if user.PasswordLegacy {
		a.migrateLegacyPassword(user)
	}
	a.syncTeacherHash(user, client.TeacherHash())

	halls, err := a.loadLectureHalls(ctx, client, user.GroupID)
	if err != nil {
		slog.Warn("lecture halls load", "error", err)
	}
	if halls == nil {
		halls = make(ktk.LectureHallMap)
	}

	callPresets := a.loadCallPresets(ctx, client)
	absenceMarks := a.loadAbsenceMarks(ctx, client)
	pairTypes := a.loadPairTypes(ctx, client)

	session := &Session{
		Client:           client,
		Halls:            halls,
		CallPresets:      callPresets,
		AbsenceMarks:     absenceMarks,
		PairTypes:        pairTypes,
		CurrentIndex:     0,
		Subgroup:         user.Subgroup,
		ShowAllSubgroups: user.ShowAllSubgroups,
		TeacherHash:      user.TeacherHash,
	}

	a.setSession(user.TelegramID, session)
	return session, nil
}

func (a *App) syncTeacherHash(user *storage.User, teacherHash string) {
	if user == nil || teacherHash == "" || user.TeacherHash == teacherHash {
		return
	}

	user.TeacherHash = teacherHash
	if a.storage == nil {
		return
	}
	if err := a.storage.SetTeacherHash(user.TelegramID, teacherHash); err != nil {
		slog.Warn("teacher hash save", "telegram_id", user.TelegramID, "error", err)
	}
}

func (a *App) migrateLegacyPassword(user *storage.User) {
	if err := a.storage.SaveUser(*user); err != nil {
		slog.Error("legacy password migration", "error", err)
	}
}

func (a *App) authClient(ctx context.Context, login, password string, groupID int) (*ktk.Client, error) {
	endpoints := a.cachedEndpoints()
	client, err := ktk.NewClient(
		a.cfg.BaseURL,
		ktk.WithDeviceName(a.cfg.KTKDeviceName),
		ktk.WithScheduleDebug(a.cfg.KTKDebugSchedule),
		ktk.WithEndpoints(endpoints),
	)
	if err != nil {
		return nil, err
	}

	requestCtx, cancel := context.WithTimeout(ctx, 20*time.Second)
	defer cancel()

	if err := client.SignIn(requestCtx, login, password); err != nil {
		return nil, err
	}

	teacherHash := client.TeacherHash()
	weekMillis := ktk.WeekStartMillis(time.Now(), a.location)
	if hasScheduleEndpoint(endpoints) && teacherHash == "" {
		return client, nil
	}

	if err := client.RefreshEndpoints(requestCtx, groupID, weekMillis, teacherHash); err != nil {
		slog.Warn("endpoint discovery", "error", err)
		return client, nil
	}
	a.cacheEndpoints(client.Endpoints())

	return client, nil
}

func (a *App) refreshSessionSchedule(ctx context.Context, user *storage.User, session *Session, targetDate time.Time) ([]ktk.ScheduleDay, int, error) {
	weekStart := ktk.WeekStart(targetDate, a.location)
	days, err := a.loadSchedule(ctx, session.Client, user.GroupID, session.TeacherHash, weekStart)
	if err != nil {
		return nil, 0, err
	}

	displayDays := days
	if user.TeacherHash == "" {
		displayDays = ktk.FilterScheduleDays(days, user.Subgroup, user.ShowAllSubgroups)
	}
	if len(displayDays) == 0 {
		return displayDays, 0, nil
	}

	currentIndex := ktk.FindDateIndex(displayDays, targetDate, a.location)
	session.AllSchedule = days
	session.Schedule = displayDays
	session.CurrentIndex = currentIndex
	session.WeekStart = weekStart
	session.WeekSelectOffset = 0
	session.Subgroup = user.Subgroup
	session.ShowAllSubgroups = user.ShowAllSubgroups
	session.TeacherHash = user.TeacherHash
	a.setSession(user.TelegramID, session)

	return displayDays, currentIndex, nil
}

func (a *App) loadScheduleForCallback(ctx context.Context, bot *telegram.Bot, chatID int64, messageID int, session *Session, targetDate time.Time) {
	user, err := a.storage.GetUser(chatID)
	if err != nil {
		a.send(ctx, chatID, "Ошибка базы данных: "+err.Error())
		return
	}
	if user == nil {
		a.send(ctx, chatID, "Сначала авторизуйся:\n/login логин пароль")
		return
	}

	loadCtx, cancel := context.WithTimeout(ctx, 15*time.Second)
	defer cancel()

	if !a.circuitBreaker.Allow() {
		slog.Warn("circuit breaker open, rejecting schedule load", "chat_id", chatID)
		a.send(ctx, chatID, "Сервер расписания временно недоступен. Попробуй через минуту.")
		return
	}

	days, _, err := a.refreshSessionSchedule(loadCtx, user, session, targetDate)
	if err != nil {
		a.circuitBreaker.RecordFailure()
		if errors.Is(err, context.DeadlineExceeded) {
			slog.Warn("schedule load timeout for callback", "chat_id", chatID)
			a.send(ctx, chatID, "Расписание загружается слишком долго. Попробуй ещё раз.")
			return
		}
		a.send(ctx, chatID, "Не удалось получить расписание. Попробуй позже.")
		return
	}
	a.circuitBreaker.RecordSuccess()
	if len(days) == 0 {
		weekStart := ktk.WeekStart(targetDate, a.location)
		session.AllSchedule = nil
		session.Schedule = nil
		session.CurrentIndex = 0
		session.WeekStart = weekStart
		session.WeekSelectOffset = 0
		session.Subgroup = user.Subgroup
		session.ShowAllSubgroups = user.ShowAllSubgroups
		session.TeacherHash = user.TeacherHash
		a.setSession(chatID, session)
		a.editEmptyWeekMessage(ctx, bot, chatID, messageID, session)
		return
	}

	session.CurrentIndex = 0
	a.setSession(chatID, session)

	if targetDate.In(a.location).Format(time.DateOnly) == time.Now().In(a.location).Format(time.DateOnly) {
		isSchoolDay := ktk.IsSchoolDay(days, targetDate, a.location)
		dayIdx := ktk.FindDateIndex(days, targetDate, a.location)
		isNonSchoolDay := isSchoolDay && ktk.IsNonSchoolDay(days[dayIdx])

		if !isSchoolDay || isNonSchoolDay {
			if user != nil {
				if a.switchToNextWeekSchedule(loadCtx, session, user.GroupID, user.TeacherHash, targetDate) && !isSchoolDay {
					session.CurrentIndex = -1
				}
				a.setSession(chatID, session)
			}
			a.editNonSchoolDayMessage(ctx, bot, chatID, messageID, session, targetDate)
			return
		}
	}

	a.editScheduleMessage(ctx, bot, chatID, messageID, session)
}

func (a *App) switchToNextWeekSchedule(ctx context.Context, session *Session, groupID int, teacherHash string, targetDate time.Time) bool {
	nextWeekStart := ktk.WeekStart(targetDate, a.location).AddDate(0, 0, 7)
	nextDays, err := a.loadSchedule(ctx, session.Client, groupID, teacherHash, nextWeekStart)
	if err != nil || len(nextDays) == 0 {
		return false
	}
	nextDisplay := nextDays
	if teacherHash == "" {
		nextDisplay = ktk.FilterScheduleDays(nextDays, session.Subgroup, session.ShowAllSubgroups)
	}
	if len(nextDisplay) == 0 {
		return false
	}
	session.Schedule = nextDisplay
	session.AllSchedule = nextDays
	session.WeekStart = nextWeekStart
	session.CurrentIndex = 0
	session.WeekSelectOffset = 0
	return true
}

func (a *App) loadSchedule(ctx context.Context, client *ktk.Client, groupID int, teacherHash string, weekStart time.Time) ([]ktk.ScheduleDay, error) {
	weekKey := weekStart.In(a.location).Format(time.DateOnly)
	if days, ok := a.scheduleCache.get(groupID, weekKey, teacherHash); ok {
		return days, nil
	}

	if client == nil {
		days, err := a.loadPersistentScheduleCache(groupID, weekKey, teacherHash)
		if err != nil {
			return nil, err
		}
		if days == nil {
			return nil, errors.New("schedule client is unavailable and cached schedule was not found")
		}
		if hasScheduledSubjects(days) {
			a.scheduleCache.set(groupID, weekKey, teacherHash, days)
		}
		return days, nil
	}

	requestCtx, cancel := context.WithTimeout(ctx, 20*time.Second)
	defer cancel()

	weekMillis := ktk.WeekStartMillis(weekStart, a.location)
	var days []ktk.ScheduleDay
	var err error
	if teacherHash != "" {
		days, err = client.GetTeacherSchedule(requestCtx, teacherHash, weekMillis)
	} else {
		days, err = client.GetSchedule(requestCtx, groupID, weekMillis)
	}
	if err == nil {
		a.cacheEndpoints(client.Endpoints())
		a.savePersistentScheduleCache(groupID, weekKey, teacherHash, days)
		if hasScheduledSubjects(days) {
			a.scheduleCache.set(groupID, weekKey, teacherHash, days)
		}
		return days, nil
	}

	cachedDays, cacheErr := a.loadPersistentScheduleCache(groupID, weekKey, teacherHash)
	if cacheErr != nil {
		slog.Warn("persistent schedule cache load", "group_id", groupID, "week_start", weekKey, "teacher", teacherHash != "", "error", cacheErr)
		return days, err
	}
	if cachedDays != nil {
		slog.Warn("using persistent schedule cache", "group_id", groupID, "week_start", weekKey, "teacher", teacherHash != "", "error", err)
		if hasScheduledSubjects(cachedDays) {
			a.scheduleCache.set(groupID, weekKey, teacherHash, cachedDays)
		}
		return cachedDays, nil
	}
	return nil, err
}

func (a *App) savePersistentScheduleCache(groupID int, weekKey string, teacherHash string, days []ktk.ScheduleDay) {
	if a.storage == nil {
		return
	}
	data, err := json.Marshal(days)
	if err != nil {
		slog.Warn("persistent schedule cache marshal", "group_id", groupID, "week_start", weekKey, "teacher", teacherHash != "", "error", err)
		return
	}
	if err := a.storage.SaveScheduleCache(storage.CachedSchedule{
		GroupID:     groupID,
		WeekStart:   weekKey,
		TeacherHash: teacherHash,
		Data:        data,
	}); err != nil {
		slog.Warn("persistent schedule cache save", "group_id", groupID, "week_start", weekKey, "teacher", teacherHash != "", "error", err)
	}
}

func (a *App) loadPersistentScheduleCache(groupID int, weekKey string, teacherHash string) ([]ktk.ScheduleDay, error) {
	if a.storage == nil {
		return nil, nil
	}
	entry, err := a.storage.GetScheduleCache(groupID, weekKey, teacherHash)
	if err != nil || entry == nil {
		return nil, err
	}
	var days []ktk.ScheduleDay
	if err := json.Unmarshal(entry.Data, &days); err != nil {
		return nil, err
	}
	return days, nil
}

func hasScheduledSubjects(days []ktk.ScheduleDay) bool {
	for _, day := range days {
		if len(day.Subjects) > 0 {
			return true
		}
	}
	return false
}

func (a *App) loadCallPresets(ctx context.Context, client *ktk.Client) ktk.CallPresetMap {
	requestCtx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()

	presets, err := client.GetCallPresets(requestCtx)
	if err != nil {
		slog.Warn("call presets load", "error", err)
		return nil
	}
	return presets
}

func (a *App) loadPairTypes(ctx context.Context, client *ktk.Client) ktk.PairTypeMap {
	requestCtx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()

	types, err := client.GetPairTypes(requestCtx)
	if err != nil {
		slog.Warn("pair types load", "error", err)
		return nil
	}
	return types
}

func (a *App) loadAbsenceMarks(ctx context.Context, client *ktk.Client) []ktk.AbsenceMark {
	requestCtx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()

	marks, err := client.GetAbsenceMarks(requestCtx)
	if err != nil {
		slog.Warn("absence marks load", "error", err)
		return nil
	}
	return marks
}

func (a *App) loadLectureHalls(ctx context.Context, client *ktk.Client, groupID int) (ktk.LectureHallMap, error) {
	requestCtx, cancel := context.WithTimeout(ctx, 20*time.Second)
	defer cancel()

	weekMillis := ktk.WeekStartMillis(time.Now(), a.location)
	halls, err := client.GetLectureHalls(requestCtx, groupID, weekMillis)
	if err == nil {
		a.cacheEndpoints(client.Endpoints())
	}
	return halls, err
}
