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
	groupID   int
	weekStart string
}

func keyFor(groupID int, weekStart string, teacherHash string) cacheKey {
	if teacherHash != "" {
		groupID = int(hashString(teacherHash))
	}
	return cacheKey{groupID, weekStart}
}

func hashString(s string) int64 {
	var h int64
	for _, c := range s {
		h = h*31 + int64(c)
	}
	return h
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

func (c *scheduleCache) get(groupID int, weekStart string, teacherHash string) ([]ktk.ScheduleDay, bool) {
	c.mu.RLock()
	defer c.mu.RUnlock()

	entry, ok := c.entries[keyFor(groupID, weekStart, teacherHash)]
	if !ok || time.Now().After(entry.expiresAt) {
		return nil, false
	}
	return entry.days, true
}

func (c *scheduleCache) set(groupID int, weekStart string, teacherHash string, days []ktk.ScheduleDay) {
	c.mu.Lock()
	defer c.mu.Unlock()

	c.entries[keyFor(groupID, weekStart, teacherHash)] = cacheEntry{
		days:      days,
		expiresAt: time.Now().Add(scheduleCacheTTL),
	}
}

func (c *scheduleCache) invalidate(groupID int) {
	c.mu.Lock()
	defer c.mu.Unlock()

	for key := range c.entries {
		if key.groupID == groupID {
			delete(c.entries, key)
		}
	}
}
