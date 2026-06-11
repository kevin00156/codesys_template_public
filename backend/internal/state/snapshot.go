// Package state holds the most recent successful read of the PLC's
// data segment, behind a RWMutex so the Modbus and (future) HTTP
// servers can read concurrently without coordinating with the
// shm-poll goroutine.
package state

import (
	"sync"
	"time"

	"codesys_dev/backend/internal/shm"
)

type Snapshot struct {
	mu        sync.RWMutex
	data      shm.PlcData
	updatedAt time.Time
	valid     bool
}

func (s *Snapshot) Update(d *shm.PlcData) {
	s.mu.Lock()
	s.data = *d
	s.updatedAt = time.Now()
	s.valid = true
	s.mu.Unlock()
}

// Read returns a copy of the latest data, the elapsed time since it
// was published, and whether any data has been received yet.
func (s *Snapshot) Read() (data shm.PlcData, age time.Duration, ok bool) {
	s.mu.RLock()
	data = s.data
	age = time.Since(s.updatedAt)
	ok = s.valid
	s.mu.RUnlock()
	return
}
