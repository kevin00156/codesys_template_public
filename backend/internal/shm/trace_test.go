package shm

import (
	"sync/atomic"
	"testing"
	"unsafe"
)

// testRing builds an in-memory plc_trace segment and returns a push function
// that mimics the Rust TraceWriter (payload stores, then WriteIdx release).
func testRing(t *testing.T, capacity uint32) (*TraceRing, func(TraceSample)) {
	t.Helper()
	size := SizeTraceHeader + int(capacity)*SizeTraceSample
	buf := make([]byte, size)
	m := mappingFromBytes(buf)

	hdr := (*TraceHeader)(m.Ptr())
	*hdr = TraceHeader{
		Magic:       PlcTraceMagic,
		Version:     PlcTraceVersion,
		SampleSize:  uint32(SizeTraceSample),
		Capacity:    capacity,
		PeriodNs:    2_000_000,
		EpochUnixNs: 1_000,
	}

	ring, err := NewTraceRing(m)
	if err != nil {
		t.Fatalf("NewTraceRing: %v", err)
	}
	var count uint64
	push := func(s TraceSample) {
		off := SizeTraceHeader + int(count&uint64(capacity-1))*SizeTraceSample
		*(*TraceSample)(unsafe.Add(m.Ptr(), off)) = s
		count++
		atomic.StoreUint64(&hdr.WriteIdx, count)
	}
	return ring, push
}

func sampleN(i uint64) TraceSample {
	var s TraceSample
	s.Cycle = i
	s.Axes[0].ActPos = float64(i)
	return s
}

func TestTraceRingReadRange(t *testing.T) {
	ring, push := testRing(t, 8)

	// Empty ring.
	samples, _, next, dropped := ring.ReadRange(0, 100)
	if len(samples) != 0 || next != 0 || dropped != 0 {
		t.Fatalf("empty ring: got %d samples, next=%d dropped=%d", len(samples), next, dropped)
	}

	for i := uint64(0); i < 5; i++ {
		push(sampleN(i))
	}
	samples, first, next, dropped := ring.ReadRange(0, 100)
	if first != 0 || next != 5 || dropped != 0 || len(samples) != 5 {
		t.Fatalf("first=%d next=%d dropped=%d len=%d", first, next, dropped, len(samples))
	}
	if samples[4] != sampleN(4) {
		t.Fatalf("sample content mismatch: %+v", samples[4])
	}

	// Cursor continuation: nothing new.
	samples, _, next, _ = ring.ReadRange(next, 100)
	if len(samples) != 0 || next != 5 {
		t.Fatalf("continuation: len=%d next=%d", len(samples), next)
	}
}

func TestTraceRingOverwrite(t *testing.T) {
	ring, push := testRing(t, 8)
	for i := uint64(0); i < 20; i++ {
		push(sampleN(i))
	}
	// The last 8 live, minus one for the mid-push guard (the reader cannot
	// tell an idle writer from one mid-push into the oldest slot).
	samples, first, next, dropped := ring.ReadRange(0, 100)
	if first != 13 || next != 20 || dropped != 13 || len(samples) != 7 {
		t.Fatalf("first=%d next=%d dropped=%d len=%d", first, next, dropped, len(samples))
	}
	if samples[0] != sampleN(13) {
		t.Fatalf("oldest surviving sample: %+v", samples[0])
	}
}

func TestTraceRingMaxCapsBatch(t *testing.T) {
	ring, push := testRing(t, 16)
	for i := uint64(0); i < 10; i++ {
		push(sampleN(i))
	}
	samples, _, next, _ := ring.ReadRange(0, 4)
	if len(samples) != 4 || next != 4 {
		t.Fatalf("len=%d next=%d", len(samples), next)
	}
	samples, _, next, _ = ring.ReadRange(next, 4)
	if samples[0] != sampleN(4) || next != 8 {
		t.Fatalf("second batch starts at %d, next=%d", samples[0].Cycle, next)
	}
}

func TestTraceRingRejectsBadHeader(t *testing.T) {
	buf := make([]byte, SizeTraceHeader+8*SizeTraceSample)
	if _, err := NewTraceRing(mappingFromBytes(buf)); err == nil {
		t.Fatal("zeroed segment must be rejected")
	}

	hdr := (*TraceHeader)(unsafe.Pointer(&buf[0]))
	*hdr = TraceHeader{Magic: PlcTraceMagic, Version: PlcTraceVersion + 1}
	if _, err := NewTraceRing(mappingFromBytes(buf)); err != ErrTraceVersion {
		t.Fatalf("want ErrTraceVersion, got %v", err)
	}

	hdr.Version = PlcTraceVersion
	hdr.SampleSize = uint32(SizeTraceSample)
	hdr.Capacity = 7 // not a power of two
	if _, err := NewTraceRing(mappingFromBytes(buf)); err == nil {
		t.Fatal("non-power-of-two capacity must be rejected")
	}
}
