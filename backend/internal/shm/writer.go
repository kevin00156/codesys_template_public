package shm

import "sync/atomic"

// WritePlcCommand publishes src to the segment under the seqlock protocol.
// Caller must initialise src.Header.Magic/Version before the first call
// (newCmdSink does this). Adding fields to PlcCommand requires no changes here.
//
// The writer — not the caller — owns Header.Seq. The bulk copy below would
// otherwise overwrite the segment's seq with src.Header.Seq (an even value),
// briefly making the segment look "stable" while the payload is still being
// written, so a reader could latch a torn snapshot. We force the copied seq
// to the in-progress (odd) value to keep the seqlock invariant: seq stays
// odd for the entire time the payload is in flux.
func WritePlcCommand(m *Mapping, src *PlcCommand) {
	dst := (*PlcCommand)(m.Ptr())
	s := atomic.LoadUint32(&dst.Header.Seq)
	odd := s + 1
	atomic.StoreUint32(&dst.Header.Seq, odd) // odd: write in progress

	tmp := *src
	tmp.Header.Seq = odd // copy carries the in-progress seq, never an even one
	*dst = tmp

	atomic.StoreUint32(&dst.Header.Seq, odd+1) // even: stable
}
