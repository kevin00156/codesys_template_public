//go:build !race

// This test deliberately races a writer against a reader on the same
// segment — that is what the seqlock is for. The payload copy is a benign
// race that -race cannot model, so the file is excluded from -race builds.

package shm

import (
	"sync"
	"sync/atomic"
	"testing"
)

// TestSeqlock_NoTornReads is a contention smoke test, not a proof. The
// writer keeps two payload fields equal on every write; a reader that ever
// observes them unequal has latched a torn snapshot. It exercises the full
// read/write protocol under real parallelism and catches gross breakage
// (seq never settling even, reader never stabilising, payload skew).
//
// It does NOT reliably reproduce the WritePlcCommand seq-clobber regression
// on its own: the reader's s1!=s2 check masks the transient even seq in all
// but a vanishingly rare interleaving (writer preempted mid-memmove for a
// whole reader cycle) that a unit test can't force. The real protection for
// that case is the writer keeping seq odd throughout the copy — see the
// comment in writer.go.
func TestSeqlock_NoTornReads(t *testing.T) {
	m, _ := cmdMapping()

	var stop atomic.Bool
	var wg sync.WaitGroup

	wg.Add(1)
	go func() {
		defer wg.Done()
		var src PlcCommand
		src.Header.Magic = PlcCommandMagic
		src.Header.Version = PlcCommandVersion
		for i := uint32(1); i <= 2_000_000; i++ {
			// Two header fields that must always agree within a single
			// snapshot (Flags is the low 16 bits of Cycle).
			src.Header.Flags = uint16(i)
			src.Header.Cycle = uint64(i)
			WritePlcCommand(m, &src)
		}
		stop.Store(true)
	}()

	torn := 0
	reads := 0
	for !stop.Load() {
		got, ok := readCmd(m)
		if !ok {
			continue
		}
		reads++
		if got.Header.Flags != uint16(got.Header.Cycle) {
			torn++
		}
	}
	wg.Wait()

	if torn != 0 {
		t.Fatalf("observed %d torn reads out of %d — seqlock invariant violated", torn, reads)
	}
	t.Logf("clean: %d consistent reads", reads)
}
