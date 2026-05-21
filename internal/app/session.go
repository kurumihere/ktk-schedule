package app

import (
	"context"
	"sync/atomic"
	"time"

	"ktk-schedule/internal/ktk"
)

type Session struct {
	Client           *ktk.Client
	Schedule         []ktk.ScheduleDay
	Halls            ktk.LectureHallMap
	CallPresets      ktk.CallPresetMap
	AbsenceMarks     []ktk.AbsenceMark
	CurrentIndex     int
	WeekStart        time.Time
	WeekSelectOffset int
	Subgroup         string
	ShowAllSubgroups bool
	lastAccessUnix   int64 // atomic
}

func (s *Session) copy() *Session {
	if s == nil {
		return nil
	}
	c := &Session{
		Client:           s.Client,
		Schedule:         make([]ktk.ScheduleDay, len(s.Schedule)),
		CurrentIndex:     s.CurrentIndex,
		WeekStart:        s.WeekStart,
		WeekSelectOffset: s.WeekSelectOffset,
		Subgroup:         s.Subgroup,
		ShowAllSubgroups: s.ShowAllSubgroups,
		lastAccessUnix:   atomic.LoadInt64(&s.lastAccessUnix),
	}
	copy(c.Schedule, s.Schedule)

	for i := range c.Schedule {
		if len(s.Schedule[i].Subjects) > 0 {
			c.Schedule[i].Subjects = make([]ktk.ScheduleItem, len(s.Schedule[i].Subjects))
			copy(c.Schedule[i].Subjects, s.Schedule[i].Subjects)
		}
	}

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

	return c
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
		new := old.copy()
		fn(new)
		new.touchLastAccess()
		if p.CompareAndSwap(old, new) {
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
