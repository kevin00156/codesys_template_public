package state

import (
	"testing"
	"time"

	"codesys_dev/backend/internal/shm"
)

func TestSnapshot_AgeTracksProducerCycle(t *testing.T) {
	var s Snapshot
	var d shm.PlcData

	if _, _, ok := s.Read(); ok {
		t.Fatal("Read ok before any Update")
	}

	d.Header.Cycle = 1
	s.Update(&d)

	// Re-polling the same frozen segment must not reset the age — the PLC is
	// dead even though the shm read keeps succeeding.
	time.Sleep(30 * time.Millisecond)
	s.Update(&d)
	if _, age, ok := s.Read(); !ok || age < 25*time.Millisecond {
		t.Fatalf("age reset by same-cycle Update: age=%v ok=%v", age, ok)
	}

	// A new producer cycle is a fresh publish.
	d.Header.Cycle = 2
	s.Update(&d)
	if _, age, _ := s.Read(); age > 20*time.Millisecond {
		t.Fatalf("age not refreshed by new cycle: %v", age)
	}
}
