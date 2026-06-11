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
