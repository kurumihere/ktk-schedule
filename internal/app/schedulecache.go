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

type cacheEntry struct {
	days      []ktk.ScheduleDay
	expiresAt time.Time
}

func newScheduleCache() *scheduleCache {
	return &scheduleCache{
		entries: make(map[cacheKey]cacheEntry),
	}
}

func (c *scheduleCache) get(groupID int, weekStart string) ([]ktk.ScheduleDay, bool) {
	c.mu.RLock()
	defer c.mu.RUnlock()

	entry, ok := c.entries[cacheKey{groupID, weekStart}]
	if !ok || time.Now().After(entry.expiresAt) {
		return nil, false
	}
	return entry.days, true
}

func (c *scheduleCache) set(groupID int, weekStart string, days []ktk.ScheduleDay) {
	c.mu.Lock()
	defer c.mu.Unlock()

	c.entries[cacheKey{groupID, weekStart}] = cacheEntry{
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
