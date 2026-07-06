package shm

import (
	"errors"
	"sync/atomic"
)

var (
	ErrSeqlockBusy     = errors.New("seqlock: writer kept us spinning")
	ErrMagicMismatch   = errors.New("magic mismatch: wrong segment or layout")
	ErrVersionMismatch = errors.New("version mismatch: rebuild PLC and Go with matching layout")
)

const seqlockMaxRetries = 100

// ReadPlcData copies the segment into dst under the seqlock protocol.
// On success, dst.Header.Magic and dst.Header.Version are guaranteed
// to match this build's expectations.
//
// Memory-ordering caveat: only Header.Seq is accessed atomically; the payload
// is a plain bulk copy. Go's atomics give acquire/release ordering, but
// nothing stops the hardware from reordering the payload's plain loads past
// the second seq load — that requires a read fence Go does not expose. On
// x86/amd64 (TSO: loads are not reordered with loads) this is sound, and
// amd64 is the only deploy target (Makefile GOARCH). Porting to ARM needs a
// fence or per-word atomic copies. The Seq accesses themselves must stay
// atomic: they also prevent the compiler from reordering across them.
func ReadPlcData(m *Mapping, dst *PlcData) error {
	src := (*PlcData)(m.Ptr())
	for i := 0; i < seqlockMaxRetries; i++ {
		s1 := atomic.LoadUint32(&src.Header.Seq)
		if s1&1 != 0 {
			continue
		}
		*dst = *src
		s2 := atomic.LoadUint32(&src.Header.Seq)
		if s1 != s2 {
			continue
		}
		if dst.Header.Magic != PlcDataMagic {
			return ErrMagicMismatch
		}
		if dst.Header.Version != PlcDataVersion {
			return ErrVersionMismatch
		}
		return nil
	}
	return ErrSeqlockBusy
}
