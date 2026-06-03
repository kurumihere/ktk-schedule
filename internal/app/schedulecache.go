package app

import (
	"sync"
	"time"

	"ktk-schedule/internal/ktk"
)

const scheduleCacheTTL = 5 * time.Minute

type scheduleCache struct {
	mu      sync.RWMutex
	entries map[cacheKey]cacheEntry
}

type cacheKey struct {
	groupID       int
	weekStart     string
	teacherHash   string
	groupSchedule bool
}

func keyFor(groupID int, weekStart string, teacherHash string, groupSchedule bool) cacheKey {
	return cacheKey{groupID, weekStart, teacherHash, groupSchedule}
}

type cacheEntry struct {
	days      []ktk.ScheduleDay
	expiresAt time.Time
}

func newScheduleCache() *scheduleCache {
	return &scheduleCache{
		entries: make(map[cacheKey]cacheEntry),
	}
}

func (c *scheduleCache) getWithMode(groupID int, weekStart string, teacherHash string, groupSchedule bool) ([]ktk.ScheduleDay, bool) {
	c.mu.RLock()
	defer c.mu.RUnlock()

	entry, ok := c.entries[keyFor(groupID, weekStart, teacherHash, groupSchedule)]
	if !ok || time.Now().After(entry.expiresAt) {
		return nil, false
	}
	return entry.days, true
}

func (c *scheduleCache) setWithMode(groupID int, weekStart string, teacherHash string, groupSchedule bool, days []ktk.ScheduleDay) {
	c.mu.Lock()
	defer c.mu.Unlock()

	c.entries[keyFor(groupID, weekStart, teacherHash, groupSchedule)] = cacheEntry{
		days:      days,
		expiresAt: time.Now().Add(scheduleCacheTTL),
	}
}

func (c *scheduleCache) invalidate(groupID int, teacherHash ...string) {
	c.mu.Lock()
	defer c.mu.Unlock()

	target := groupID
	targetTeacherHash := ""
	if len(teacherHash) > 0 && teacherHash[0] != "" {
		targetTeacherHash = teacherHash[0]
	}

	for key := range c.entries {
		if targetTeacherHash != "" && key.teacherHash == targetTeacherHash {
			delete(c.entries, key)
			continue
		}
		if targetTeacherHash == "" && key.groupID == target {
			delete(c.entries, key)
		}
	}
}
