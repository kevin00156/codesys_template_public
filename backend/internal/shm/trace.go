// The plc_trace segment: a single-producer broadcast ring the Rust daemon
// fills with one fixed TraceSample per control cycle (the data source for
// the HMI's watch/trace panels). Mirror of rust/shm-bridge/src/trace.rs —
// layout equality is pinned by vet.go and the shared golden fixture
// testdata/plc_trace_v1.bin.
//
// Unlike plc_data this is not a seqlock: TraceHeader.WriteIdx is a monotonic
// count of samples ever written, slot i%Capacity holds sample i. A reader
// copies a range, re-loads WriteIdx and discards anything the writer may
// have lapped during the copy. Like ReadPlcData, the bulk struct copy is
// formally racy but validated after the fact (x86-TSO posture).

package shm

import (
	"errors"
	"fmt"
	"sync/atomic"
	"unsafe"
)

const (
	PlcTraceMagic   uint32 = 0x504C4354 // 'PLCT'
	PlcTraceVersion uint16 = 1

	NamePlcTrace = "plc_trace"
)

// TraceHeader is the first 64 bytes of the trace segment. Everything except
// WriteIdx is written once at creation.
type TraceHeader struct {
	Magic       uint32
	Version     uint16
	Flags       uint16
	SampleSize  uint32 // sizeof(TraceSample) — verified before trusting offsets
	Capacity    uint32 // ring capacity in samples, always a power of two
	PeriodNs    uint64 // nominal cycle period
	EpochUnixNs uint64 // CLOCK_REALTIME at creation; + TMonoNs = wall clock
	WriteIdx    uint64 // monotonic count of samples ever written (atomic)
	_           [3]uint64
}

// TraceAxisSample is the per-axis slice of a sample. 56 bytes.
// DriveStatus: 0=Offline 1=Disabled 2=Enabling 3=Enabled 4=QuickStop 5=Fault.
// IOBits: b0=pos_limit b1=neg_limit b2=homed b3=out.enable b4=out.fault_reset.
type TraceAxisSample struct {
	ActPos      float64
	ActVel      float64
	SetPos      float64
	SetVel      float64
	Step        int32  // enumAxisControl_Step (same as AxisState.Step)
	Flags       uint32 // same bits as AxisState.Flags
	ErrorID     int32
	FaultCode   uint32 // backend-native fault code (not in PlcData)
	DriveStatus uint32
	IOBits      uint32
}

// TraceSample is one control cycle. 256 bytes.
// BusState: 0=Init 1=PreOp 2=SafeOp 3=Op.
// StatusBits: b0=exchange_error b1=cmd_fresh b2=cmd_valid b3=cycle_overrun b4=wkc_error.
type TraceSample struct {
	Cycle      uint64 // plc_data header cycle counter at publish time
	TMonoNs    uint64 // monotonic ns since segment creation
	PeriodNs   uint32 // measured wake-to-wake period
	ExchangeNs uint32 // bus exchange duration
	BusState   uint8
	StatusBits uint8
	_          uint16
	RunState   uint32
	Axes       [4]TraceAxisSample
}

const (
	SizeTraceHeader = int(unsafe.Sizeof(TraceHeader{}))
	SizeTraceSample = int(unsafe.Sizeof(TraceSample{}))
)

var (
	ErrTraceMagic    = errors.New("trace: magic mismatch — wrong segment or layout")
	ErrTraceVersion  = errors.New("trace: version mismatch — rebuild daemon and bridge together")
	ErrTraceGeometry = errors.New("trace: header geometry invalid")
)

// TraceRing is a validating reader over a mapped plc_trace segment.
type TraceRing struct {
	m        *Mapping
	capacity uint64
}

// NewTraceRingFromBytes wraps an in-memory segment image — for tests (see
// mappingFromBytes) and offline tools reading a dumped segment.
func NewTraceRingFromBytes(b []byte) (*TraceRing, error) {
	return NewTraceRing(mappingFromBytes(b))
}

// NewTraceRing validates the header of an already-mapped segment.
func NewTraceRing(m *Mapping) (*TraceRing, error) {
	if m.Size() < SizeTraceHeader {
		return nil, fmt.Errorf("%w: segment %d B < header %d B", ErrTraceGeometry, m.Size(), SizeTraceHeader)
	}
	hdr := (*TraceHeader)(m.Ptr())
	if hdr.Magic != PlcTraceMagic {
		return nil, ErrTraceMagic
	}
	if hdr.Version != PlcTraceVersion {
		return nil, ErrTraceVersion
	}
	cap := hdr.Capacity
	if hdr.SampleSize != uint32(SizeTraceSample) ||
		cap == 0 || cap&(cap-1) != 0 ||
		m.Size() < SizeTraceHeader+int(cap)*SizeTraceSample {
		return nil, fmt.Errorf("%w: sampleSize=%d capacity=%d segment=%d B",
			ErrTraceGeometry, hdr.SampleSize, cap, m.Size())
	}
	return &TraceRing{m: m, capacity: uint64(cap)}, nil
}

// Header returns a copy of the segment header (WriteIdx loaded atomically).
func (r *TraceRing) Header() TraceHeader {
	src := (*TraceHeader)(r.m.Ptr())
	h := *src
	h.WriteIdx = atomic.LoadUint64(&src.WriteIdx)
	return h
}

func (r *TraceRing) Capacity() uint64 { return r.capacity }

func (r *TraceRing) Close() error { return r.m.Close() }

func (r *TraceRing) slot(i uint64) *TraceSample {
	off := SizeTraceHeader + int(i&(r.capacity-1))*SizeTraceSample
	return (*TraceSample)(unsafe.Add(r.m.Ptr(), off))
}

// ReadRange copies samples [since, WriteIdx) — at most max — out of the ring.
// Returns the samples, the index of the first one, the cursor to pass as the
// next `since`, and how many requested samples were lost to ring overwrite.
//
// Overwrite protocol (mirror of TraceReader::read_range in Rust): load
// WriteIdx (w1), clamp to the live window, copy, re-load WriteIdx (w2) and
// discard everything <= w2-Capacity — WriteIdx==w2 means the writer may be
// mid-push w2, so sample w2-Capacity (sharing that slot) may be torn.
func (r *TraceRing) ReadRange(since uint64, max int) (samples []TraceSample, first, next, dropped uint64) {
	idx := &(*TraceHeader)(r.m.Ptr()).WriteIdx

	w1 := atomic.LoadUint64(idx)
	oldest := uint64(0)
	if w1 > r.capacity {
		oldest = w1 - r.capacity
	}
	start := since
	if start < oldest {
		start = oldest
	}
	end := w1
	if max >= 0 && end > start+uint64(max) {
		end = start + uint64(max)
	}
	if start >= end {
		next = w1
		if since > next {
			next = since
		}
		return nil, w1, next, start - min64(since, start)
	}

	samples = make([]TraceSample, 0, end-start)
	for i := start; i < end; i++ {
		samples = append(samples, *r.slot(i)) // racy, validated below
	}
	w2 := atomic.LoadUint64(idx)

	validFrom := start
	if w2+1 > r.capacity && w2+1-r.capacity > validFrom {
		validFrom = w2 + 1 - r.capacity
	}
	if torn := validFrom - start; torn > 0 {
		if torn >= uint64(len(samples)) {
			samples = nil
		} else {
			samples = samples[torn:]
		}
	}
	return samples, validFrom, end, validFrom - min64(since, validFrom)
}

func min64(a, b uint64) uint64 {
	if a < b {
		return a
	}
	return b
}
