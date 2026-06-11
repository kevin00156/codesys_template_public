package shm

import (
	"sync/atomic"
	"testing"
	"unsafe"
)

// dataMapping returns an in-memory Mapping backed by a freshly allocated,
// 8-byte-aligned PlcData (atomics on Header.Seq and the float64 fields need
// proper alignment). The returned pointer aliases the same memory.
func dataMapping() (*Mapping, *PlcData) {
	pd := new(PlcData)
	b := unsafe.Slice((*byte)(unsafe.Pointer(pd)), SizePlcData)
	return mappingFromBytes(b), pd
}

func cmdMapping() (*Mapping, *PlcCommand) {
	pc := new(PlcCommand)
	b := unsafe.Slice((*byte)(unsafe.Pointer(pc)), SizePlcCommand)
	return mappingFromBytes(b), pc
}

// readCmd is a test-local seqlock reader for the command segment (Go never
// reads plc_cmd in production — the PLC does — so there is no ReadPlcCommand).
func readCmd(m *Mapping) (PlcCommand, bool) {
	src := (*PlcCommand)(m.Ptr())
	for i := 0; i < seqlockMaxRetries; i++ {
		s1 := atomic.LoadUint32(&src.Header.Seq)
		if s1&1 != 0 {
			continue
		}
		dst := *src
		s2 := atomic.LoadUint32(&src.Header.Seq)
		if s1 == s2 {
			return dst, true
		}
	}
	return PlcCommand{}, false
}

func TestReadPlcData_RoundTrip(t *testing.T) {
	m, seg := dataMapping()
	seg.Header.Magic = PlcDataMagic
	seg.Header.Version = PlcDataVersion
	seg.Header.Seq = 2 // even == stable
	seg.Header.Cycle = 42

	var dst PlcData
	if err := ReadPlcData(m, &dst); err != nil {
		t.Fatalf("ReadPlcData: %v", err)
	}
	if dst.Header.Cycle != 42 {
		t.Errorf("header mismatch: %+v", dst.Header)
	}
}

func TestReadPlcData_Rejects(t *testing.T) {
	t.Run("magic", func(t *testing.T) {
		m, seg := dataMapping()
		seg.Header.Magic = 0xBADBAD
		seg.Header.Version = PlcDataVersion
		seg.Header.Seq = 2
		var dst PlcData
		if err := ReadPlcData(m, &dst); err != ErrMagicMismatch {
			t.Errorf("got %v want ErrMagicMismatch", err)
		}
	})

	t.Run("version", func(t *testing.T) {
		m, seg := dataMapping()
		seg.Header.Magic = PlcDataMagic
		seg.Header.Version = PlcDataVersion + 99
		seg.Header.Seq = 2
		var dst PlcData
		if err := ReadPlcData(m, &dst); err != ErrVersionMismatch {
			t.Errorf("got %v want ErrVersionMismatch", err)
		}
	})

	t.Run("busy", func(t *testing.T) {
		m, seg := dataMapping()
		seg.Header.Magic = PlcDataMagic
		seg.Header.Version = PlcDataVersion
		seg.Header.Seq = 1 // odd forever -> writer perpetually in progress
		var dst PlcData
		if err := ReadPlcData(m, &dst); err != ErrSeqlockBusy {
			t.Errorf("got %v want ErrSeqlockBusy", err)
		}
	})
}

func TestWritePlcCommand_RoundTrip(t *testing.T) {
	m, _ := cmdMapping()

	var src PlcCommand
	src.Header.Magic = PlcCommandMagic
	src.Header.Version = PlcCommandVersion
	src.Header.Seq = 999 // garbage: the writer owns seq and must ignore this
	src.Header.Cycle = 7

	WritePlcCommand(m, &src)

	got, ok := readCmd(m)
	if !ok {
		t.Fatal("readCmd: seqlock never stabilised")
	}
	if got.Header.Cycle != 7 {
		t.Errorf("payload mismatch: %+v", got.Header)
	}
	if got.Header.Magic != PlcCommandMagic || got.Header.Version != PlcCommandVersion {
		t.Errorf("header not copied: %+v", got.Header)
	}
	// Writer controls seq: first write must land on the first even value,
	// not anything derived from src.Header.Seq (999).
	if got.Header.Seq != 2 {
		t.Errorf("seq: got %d want 2 (writer must ignore src.Header.Seq)", got.Header.Seq)
	}

	// Second write advances seq by exactly 2 and stays even.
	src.Header.Cycle = 8
	WritePlcCommand(m, &src)
	got2, ok := readCmd(m)
	if !ok {
		t.Fatal("readCmd: seqlock never stabilised on 2nd write")
	}
	if got2.Header.Seq != 4 {
		t.Errorf("seq after 2nd write: got %d want 4", got2.Header.Seq)
	}
	if got2.Header.Cycle != 8 {
		t.Errorf("2nd payload: got %d want 8", got2.Header.Cycle)
	}
}
