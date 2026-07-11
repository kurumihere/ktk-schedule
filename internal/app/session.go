package app

import (
	"context"
	"ktk-schedule/internal/logger"
	"sync/atomic"
	"time"

	"ktk-schedule/internal/ktk"
)

type Session struct {
	Client             *ktk.Client
	AllSchedule        []ktk.ScheduleDay
	Schedule           []ktk.ScheduleDay
	Halls              ktk.LectureHallMap
	CallPresets        ktk.CallPresetMap
	AbsenceMarks       []ktk.AbsenceMark
	PairTypes          ktk.PairTypeMap
	homeworkCache      map[int]int
	documentCache      map[int]ktk.DocumentMetadata
	CurrentIndex       int
	WeekStart          time.Time
	WeekSelectOffset   int
	Subgroup           string
	ShowAllSubgroups   bool
	TeacherHash        string
	ViewingGroupID     int
	AwaitingGroupInput bool
	lastAccessUnix     int64
}

func (s *Session) copy() *Session {
	if s == nil {
		return nil
	}
	c := &Session{
		Client:             s.Client,
		AllSchedule:        copyScheduleDays(s.AllSchedule),
		Schedule:           make([]ktk.ScheduleDay, len(s.Schedule)),
		CurrentIndex:       s.CurrentIndex,
		WeekStart:          s.WeekStart,
		WeekSelectOffset:   s.WeekSelectOffset,
		Subgroup:           s.Subgroup,
		ShowAllSubgroups:   s.ShowAllSubgroups,
		TeacherHash:        s.TeacherHash,
		ViewingGroupID:     s.ViewingGroupID,
		AwaitingGroupInput: s.AwaitingGroupInput,
		lastAccessUnix:     atomic.LoadInt64(&s.lastAccessUnix),
	}
	copy(c.Schedule, s.Schedule)

	copyScheduleSubjects(c.Schedule, s.Schedule)

	if s.Halls != nil {
		c.Halls = make(ktk.LectureHallMap, len(s.Halls))
		for k, v := range s.Halls {
			c.Halls[k] = v
		}
	}

	if s.CallPresets != nil {
		c.CallPresets = make(ktk.CallPresetMap, len(s.CallPresets))
		for k, v := range s.CallPresets {
			c.CallPresets[k] = v
		}
	}

	if s.AbsenceMarks != nil {
		c.AbsenceMarks = make([]ktk.AbsenceMark, len(s.AbsenceMarks))
		copy(c.AbsenceMarks, s.AbsenceMarks)
	}

	if s.PairTypes != nil {
		c.PairTypes = make(ktk.PairTypeMap, len(s.PairTypes))
		for k, v := range s.PairTypes {
			c.PairTypes[k] = v
		}
	}

	if s.homeworkCache != nil {
		c.homeworkCache = make(map[int]int, len(s.homeworkCache))
		for k, v := range s.homeworkCache {
			c.homeworkCache[k] = v
		}
	}

	if s.documentCache != nil {
		c.documentCache = make(map[int]ktk.DocumentMetadata, len(s.documentCache))
		for k, v := range s.documentCache {
			c.documentCache[k] = v
		}
	}

	return c
}

func copyScheduleDays(days []ktk.ScheduleDay) []ktk.ScheduleDay {
	if days == nil {
		return nil
	}
	copied := make([]ktk.ScheduleDay, len(days))
	copy(copied, days)
	copyScheduleSubjects(copied, days)
	return copied
}

func copyScheduleSubjects(dst, src []ktk.ScheduleDay) {
	for i := range dst {
		if len(src[i].Subjects) > 0 {
			dst[i].Subjects = make([]ktk.ScheduleItem, len(src[i].Subjects))
			copy(dst[i].Subjects, src[i].Subjects)
		}
	}
}

func (s *Session) getHomeworkFileID(ctx context.Context, sheet int) (fileID int, ok bool) {
	if s.Client == nil {
		return 0, false
	}
	if s.homeworkCache != nil {
		if id, cached := s.homeworkCache[sheet]; cached {
			return id, true
		}
	}
	sub, err := s.Client.GetHomeworkSubmission(ctx, sheet)
	if err != nil {
		logger.Debug("Could not fetch homework submission for sheet %d: %v", sheet, err)
		return 0, false
	}
	if s.homeworkCache == nil {
		s.homeworkCache = make(map[int]int)
	}
	if sub.FileID != nil {
		s.homeworkCache[sheet] = *sub.FileID
		return *sub.FileID, true
	}
	s.homeworkCache[sheet] = 0
	return 0, true
}

func (s *Session) cachedDocumentMetadata(docID int) (ktk.DocumentMetadata, bool) {
	if s.documentCache == nil {
		return ktk.DocumentMetadata{}, false
	}
	meta, ok := s.documentCache[docID]
	return meta, ok
}

func (s *Session) cacheDocumentMetadata(meta ktk.DocumentMetadata) {
	if meta.ID == 0 {
		return
	}
	if s.documentCache == nil {
		s.documentCache = make(map[int]ktk.DocumentMetadata)
	}
	s.documentCache[meta.ID] = meta
}

func (s *Session) lastAccess() time.Time {
	return time.Unix(atomic.LoadInt64(&s.lastAccessUnix), 0)
}

func (s *Session) touchLastAccess() {
	atomic.StoreInt64(&s.lastAccessUnix, time.Now().Unix())
}

func (a *App) getSession(telegramID int64) *Session {
	ptr, ok := a.sessions.Load(telegramID)
	if !ok {
		return nil
	}
	s := ptr.(*atomic.Pointer[Session]).Load()
	if s == nil {
		return nil
	}
	s.touchLastAccess()
	return s.copy()
}

func (a *App) setSession(telegramID int64, session *Session) {
	if session == nil {
		return
	}
	session.touchLastAccess()
	ptr, _ := a.sessions.LoadOrStore(telegramID, &atomic.Pointer[Session]{})
	ptr.(*atomic.Pointer[Session]).Store(session)
}

func (a *App) deleteSession(telegramID int64) {
	a.sessions.Delete(telegramID)
}

func (a *App) modifySession(telegramID int64, fn func(*Session)) {
	ptr, ok := a.sessions.Load(telegramID)
	if !ok {
		return
	}
	p := ptr.(*atomic.Pointer[Session])
	for {
		old := p.Load()
		if old == nil {
			return
		}
		newSession := old.copy()
		fn(newSession)
		newSession.touchLastAccess()
		if p.CompareAndSwap(old, newSession) {
			return
		}
	}
}

func (a *App) sessionCount() int {
	var n int
	a.sessions.Range(func(_, _ any) bool {
		n++
		return true
	})
	return n
}

func (a *App) sessionCleanupLoop(ctx context.Context) {
	ticker := time.NewTicker(sessionCleanupInterval)
	defer ticker.Stop()

	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			a.cleanupSessions()
		}
	}
}

func (a *App) cleanupSessions() {
	now := time.Now()
	a.sessions.Range(func(key, value any) bool {
		ptr := value.(*atomic.Pointer[Session])
		s := ptr.Load()
		if s != nil && now.Sub(s.lastAccess()) > sessionMaxAge {
			a.sessions.Delete(key)
		}
		return true
	})
}
