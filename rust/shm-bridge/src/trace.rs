//! The `plc_trace` segment: a single-producer broadcast ring the daemon fills
//! with one fixed-layout [`TraceSample`] per control cycle. This is the data
//! source for the HMI's watch/trace panels (CODESYS trace parity) — the Go
//! bridge reads it and serves HTTP; nothing here is on the command path.
//!
//! Unlike `plc_data`/`plc_cmd` this segment is *not* a seqlock: the writer
//! never waits and never retries. `TraceHeader.write_idx` is a monotonic
//! count of samples ever written; slot `i % capacity` holds sample `i`.
//! Readers copy a range, then re-load `write_idx` and discard any sample the
//! writer may have lapped during the copy (`idx < write_idx - capacity`).
//! Tearing is therefore *detected by the reader*, not prevented by the
//! writer — that is what keeps the RT write path wait-free: ~32 relaxed
//! stores plus one release store, no CAS, no branches that can spin.
//!
//! Memory model (mirrors seqlock.rs): payload words are relaxed `AtomicU64`
//! accesses, the `write_idx` release store orders them for any reader that
//! acquire-loads it. On x86-64 everything compiles to plain MOVs.
//!
//! The Go mirror lives in `backend/internal/shm/trace.go`; layout equality is
//! pinned by compile-time asserts here, size pins in Go's `vet.go`, and the
//! shared golden fixture `testdata/plc_trace_v1.bin`. Field-order changes
//! MUST bump [`PLC_TRACE_VERSION`] on both sides; ring capacity is *data*
//! (`TraceHeader.capacity`), not layout, and needs no bump.

use core::mem::{offset_of, size_of};
use core::sync::atomic::{fence, AtomicU64, Ordering};
use std::sync::Arc;

use crate::mapping::Mapping;

pub const PLC_TRACE_MAGIC: u32 = 0x504C_4354; // 'PLCT'
pub const PLC_TRACE_VERSION: u16 = 1;
pub const NAME_PLC_TRACE: &str = "plc_trace";

/// First 64 bytes of the trace segment. `write_idx` is owned by an
/// `AtomicU64` view (offset pinned below); everything else is written once
/// at creation and read-only afterwards.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TraceHeader {
    pub magic: u32,
    pub version: u16,
    pub flags: u16,
    /// `size_of::<TraceSample>()` — readers verify before trusting offsets.
    pub sample_size: u32,
    /// Ring capacity in samples; always a power of two.
    pub capacity: u32,
    /// Nominal cycle period (config), ns. Actual per-cycle timing is in the
    /// samples themselves.
    pub period_ns: u64,
    /// CLOCK_REALTIME at segment creation; `epoch_unix_ns + t_mono_ns` maps a
    /// sample to wall-clock time.
    pub epoch_unix_ns: u64,
    /// Monotonic count of samples ever written (atomic).
    pub write_idx: u64,
    pub _pad: [u64; 3],
}

/// Per-axis slice of a sample. 56 bytes.
///
/// Encodings (the daemon fills these; shm-bridge stays fieldbus-agnostic):
/// `drive_status`: 0=Offline 1=Disabled 2=Enabling 3=Enabled 4=QuickStop
/// 5=Fault (fieldbus-api `DriveStatus` order).
/// `io_bits`: b0=pos_limit b1=neg_limit b2=homed b3=out.enable
/// b4=out.fault_reset.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TraceAxisSample {
    pub act_pos: f64,
    pub act_vel: f64,
    pub set_pos: f64,
    pub set_vel: f64,
    /// `enumAxisControl_Step` (same as `AxisState.step`).
    pub step: i32,
    /// Same bits as `AxisState.flags`.
    pub flags: u32,
    pub error_id: i32,
    /// Backend-native fault code (not in `PlcData` today).
    pub fault_code: u32,
    pub drive_status: u32,
    pub io_bits: u32,
}

/// One control cycle. 256 bytes exactly (32 u64 words).
///
/// Encodings: `bus_state`: 0=Init 1=PreOp 2=SafeOp 3=Op.
/// `status_bits`: b0=exchange_error b1=cmd_fresh b2=cmd_valid.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TraceSample {
    /// `plc_data` header cycle counter at publish time.
    pub cycle: u64,
    /// Monotonic ns since segment creation (pairs with `epoch_unix_ns`).
    pub t_mono_ns: u64,
    /// Measured wake-to-wake period, ns (jitter is the spread of this).
    pub period_ns: u32,
    /// `bus.exchange` duration, ns.
    pub exchange_ns: u32,
    pub bus_state: u8,
    pub status_bits: u8,
    pub _pad: u16,
    pub run_state: u32,
    pub axes: [TraceAxisSample; 4],
}

pub const SIZE_TRACE_HEADER: usize = size_of::<TraceHeader>();
pub const SIZE_TRACE_SAMPLE: usize = size_of::<TraceSample>();
const SAMPLE_WORDS: usize = SIZE_TRACE_SAMPLE / 8;

/// Total segment size for a given ring capacity.
pub fn segment_size(capacity: u32) -> usize {
    SIZE_TRACE_HEADER + capacity as usize * SIZE_TRACE_SAMPLE
}

/// Ring capacity (power of two) covering at least `seconds` at `period`.
pub fn capacity_for(seconds: u64, period_ns: u64) -> u32 {
    let samples = (seconds.max(1) * 1_000_000_000).div_ceil(period_ns.max(1));
    u32::try_from(samples.next_power_of_two()).unwrap_or(1 << 31)
}

const WRITE_IDX_OFFSET: usize = offset_of!(TraceHeader, write_idx);

fn write_idx_atomic(m: &Mapping) -> &AtomicU64 {
    debug_assert!(m.len() >= SIZE_TRACE_HEADER);
    // Safety: offset 32 is in bounds and 8-aligned (mapping is 8-aligned);
    // every access to this word goes through this atomic view.
    unsafe { AtomicU64::from_ptr(m.ptr().add(WRITE_IDX_OFFSET) as *mut u64) }
}

/// View a sample as raw bytes (golden fixtures, encoders).
pub fn sample_as_bytes(s: &TraceSample) -> &[u8] {
    // Safety: repr(C) with explicit padding only — every byte is initialised.
    unsafe { core::slice::from_raw_parts(s as *const TraceSample as *const u8, SIZE_TRACE_SAMPLE) }
}

/// Decode a sample from raw bytes (length must match exactly).
pub fn sample_from_bytes(b: &[u8]) -> TraceSample {
    assert_eq!(b.len(), SIZE_TRACE_SAMPLE, "fixture size mismatch");
    // Safety: any bit pattern is a valid TraceSample (ints/floats only).
    unsafe { (b.as_ptr() as *const TraceSample).read_unaligned() }
}

/// Sole writer of the trace ring. Owned by the daemon's cycle thread.
/// Holds the mapping behind an `Arc` so tests (and future in-process tools)
/// can read the same ring through a [`TraceReader`].
pub struct TraceWriter {
    map: Arc<Mapping>,
    capacity: u32,
    /// Local copy of the sample count — the writer is the only writer, so it
    /// never needs to read `write_idx` back from shm.
    count: u64,
}

impl TraceWriter {
    /// Wrap a fresh (zeroed) mapping and initialise the header. `capacity`
    /// must be a power of two and the mapping at least [`segment_size`] long.
    ///
    /// Pre-touches every page so the RT loop never takes a first-touch fault
    /// (pairs with `mlockall(MCL_FUTURE)` in the daemon).
    pub fn new(m: Mapping, capacity: u32, period_ns: u64, epoch_unix_ns: u64) -> TraceWriter {
        assert!(capacity.is_power_of_two(), "trace capacity must be 2^n");
        assert!(m.len() >= segment_size(capacity), "trace segment too small");

        // Pre-touch before publishing the magic: no reader trusts the
        // segment yet, so plain writes are fine here.
        let len = segment_size(capacity);
        for off in (0..len).step_by(4096) {
            // Safety: in bounds (asserted above).
            unsafe { m.ptr().add(off).write_volatile(0) };
        }

        let hdr = TraceHeader {
            magic: PLC_TRACE_MAGIC,
            version: PLC_TRACE_VERSION,
            flags: 0,
            sample_size: SIZE_TRACE_SAMPLE as u32,
            capacity,
            period_ns,
            epoch_unix_ns,
            write_idx: 0,
            _pad: [0; 3],
        };
        // Safety: header fits (asserted), mapping is 8-aligned; readers only
        // trust the header after seeing the magic, which this write makes
        // visible before any sample exists (write_idx == 0).
        unsafe { (m.ptr() as *mut TraceHeader).write(hdr) };
        fence(Ordering::Release);

        TraceWriter {
            map: Arc::new(m),
            capacity,
            count: 0,
        }
    }

    /// Shared view of the underlying mapping (for a same-process reader).
    pub fn mapping(&self) -> Arc<Mapping> {
        Arc::clone(&self.map)
    }

    /// Append one sample. Wait-free: 32 relaxed stores + 1 release store.
    pub fn push(&mut self, s: &TraceSample) {
        let slot = (self.count & (self.capacity as u64 - 1)) as usize;
        let off = SIZE_TRACE_HEADER + slot * SIZE_TRACE_SAMPLE;
        let src = s as *const TraceSample as *const u64;
        // Safety: slot is in bounds (capacity asserted in new()), 8-aligned;
        // all concurrent access to sample words is atomic.
        let dst = unsafe { self.map.ptr().add(off) } as *mut u64;
        for i in 0..SAMPLE_WORDS {
            // Safety: in bounds, 8-aligned, atomic.
            unsafe {
                AtomicU64::from_ptr(dst.add(i)).store(src.add(i).read(), Ordering::Relaxed)
            };
        }
        self.count += 1;
        write_idx_atomic(&self.map).store(self.count, Ordering::Release);
    }

    /// Samples written so far (== the shm `write_idx`).
    pub fn count(&self) -> u64 {
        self.count
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraceReadError {
    MagicMismatch,
    VersionMismatch,
    /// `sample_size`/`capacity` in the header disagree with this build or
    /// with the mapping's length.
    Geometry,
}

impl core::fmt::Display for TraceReadError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::MagicMismatch => write!(f, "trace: magic mismatch"),
            Self::VersionMismatch => write!(f, "trace: version mismatch"),
            Self::Geometry => write!(f, "trace: header geometry invalid"),
        }
    }
}

impl std::error::Error for TraceReadError {}

/// Result of one [`TraceReader::read_range`] call.
pub struct TraceRange {
    /// Index of the first returned sample.
    pub first: u64,
    /// Pass this as the next `since` to continue without gaps.
    pub next: u64,
    /// Samples the reader lost to ring overwrite (requested but no longer
    /// available, or discarded by the post-copy lap check).
    pub dropped: u64,
}

/// Validating reader for tests and tools (the Go bridge has its own mirror).
pub struct TraceReader {
    map: Arc<Mapping>,
    capacity: u32,
}

impl TraceReader {
    pub fn new(map: Arc<Mapping>) -> Result<TraceReader, TraceReadError> {
        if map.len() < SIZE_TRACE_HEADER {
            return Err(TraceReadError::Geometry);
        }
        // Safety: header fits; concurrent writer only mutates write_idx,
        // which this read tolerates (validated fields are written once).
        let hdr = unsafe { (map.ptr() as *const TraceHeader).read() };
        if hdr.magic != PLC_TRACE_MAGIC {
            return Err(TraceReadError::MagicMismatch);
        }
        if hdr.version != PLC_TRACE_VERSION {
            return Err(TraceReadError::VersionMismatch);
        }
        if hdr.sample_size != SIZE_TRACE_SAMPLE as u32
            || !hdr.capacity.is_power_of_two()
            || map.len() < segment_size(hdr.capacity)
        {
            return Err(TraceReadError::Geometry);
        }
        Ok(TraceReader {
            capacity: hdr.capacity,
            map,
        })
    }

    pub fn header(&self) -> TraceHeader {
        // Safety: validated in new(); write_idx may race, any value is valid.
        unsafe { (self.map.ptr() as *const TraceHeader).read() }
    }

    /// Copy samples `[since, write_idx)` (at most `max`) into `out`.
    ///
    /// Overwrite protocol: acquire-load `write_idx` (w1), clamp the range to
    /// the live window, copy, re-load `write_idx` (w2) and discard everything
    /// `< w2 - capacity` — those slots may have been rewritten mid-copy.
    pub fn read_range(&self, since: u64, max: usize, out: &mut Vec<TraceSample>) -> TraceRange {
        out.clear();
        let cap = self.capacity as u64;
        let idx = write_idx_atomic(&self.map);

        let w1 = idx.load(Ordering::Acquire);
        let oldest = w1.saturating_sub(cap);
        let start = since.max(oldest);
        let end = w1.min(start + max as u64);
        if start >= end {
            return TraceRange {
                first: w1,
                next: w1.max(since),
                dropped: start.saturating_sub(since),
            };
        }

        out.reserve(usize::try_from(end - start).unwrap_or(0));
        for i in start..end {
            let slot = (i & (cap - 1)) as usize;
            let off = SIZE_TRACE_HEADER + slot * SIZE_TRACE_SAMPLE;
            let mut s = TraceSample::default();
            let dst = &mut s as *mut TraceSample as *mut u64;
            // Safety: in bounds (geometry validated), 8-aligned, atomic.
            let src = unsafe { self.map.ptr().add(off) } as *const u64;
            for w in 0..SAMPLE_WORDS {
                unsafe {
                    dst.add(w).write(
                        AtomicU64::from_ptr(src.add(w) as *mut u64).load(Ordering::Relaxed),
                    )
                };
            }
            out.push(s);
        }
        fence(Ordering::Acquire); // sample loads complete before the lap check
        let w2 = idx.load(Ordering::Relaxed);

        // Discard samples the writer may have lapped during the copy.
        // `write_idx == w2` means pushes `0..w2` are complete and the writer
        // may be *mid-push* `w2` (payload stores precede the index store), so
        // sample `w2 - cap` — sharing that slot — may be torn: the newest
        // provably-clean sample is `w2 - cap + 1`. Like the Go seqlock
        // reader, the boundary argument leans on TSO store visibility
        // (x86-64: observing push q's payload implies w2 >= q).
        let valid_from = start.max((w2 + 1).saturating_sub(cap));
        let torn = usize::try_from(valid_from - start).unwrap_or(out.len());
        if torn > 0 {
            out.drain(..torn.min(out.len()));
        }
        TraceRange {
            first: valid_from,
            next: end,
            dropped: valid_from.saturating_sub(since.min(valid_from)),
        }
    }
}

// ─── Compile-time layout pins (counterpart of Go's vet.go) ───────────────────

const _: () = assert!(size_of::<TraceHeader>() == 64);
const _: () = assert!(size_of::<TraceAxisSample>() == 56);
const _: () = assert!(size_of::<TraceSample>() == 256);
const _: () = assert!(size_of::<TraceSample>() % 8 == 0);

const _: () = assert!(offset_of!(TraceHeader, sample_size) == 8);
const _: () = assert!(offset_of!(TraceHeader, capacity) == 12);
const _: () = assert!(offset_of!(TraceHeader, period_ns) == 16);
const _: () = assert!(offset_of!(TraceHeader, epoch_unix_ns) == 24);
const _: () = assert!(offset_of!(TraceHeader, write_idx) == 32);

const _: () = assert!(offset_of!(TraceSample, period_ns) == 16);
const _: () = assert!(offset_of!(TraceSample, bus_state) == 24);
const _: () = assert!(offset_of!(TraceSample, run_state) == 28);
const _: () = assert!(offset_of!(TraceSample, axes) == 32);
const _: () = assert!(offset_of!(TraceAxisSample, step) == 32);
const _: () = assert!(offset_of!(TraceAxisSample, io_bits) == 52);

#[cfg(test)]
mod tests {
    use super::*;

    fn writer(cap: u32) -> TraceWriter {
        TraceWriter::new(Mapping::in_memory(segment_size(cap)), cap, 2_000_000, 1_000)
    }

    fn sample(i: u64) -> TraceSample {
        let mut s = TraceSample::default();
        s.cycle = i;
        s.t_mono_ns = i * 2_000_000;
        s.axes[0].act_pos = i as f64;
        s
    }

    /// Reader over the same memory as the writer (the real reader is the Go
    /// bridge mapping the same /dev/shm segment).
    fn reader_of(w: &TraceWriter) -> TraceReader {
        TraceReader::new(w.mapping()).expect("valid header")
    }

    #[test]
    fn capacity_helper() {
        assert_eq!(capacity_for(60, 2_000_000), 32768); // 30k → 2^15
        assert_eq!(capacity_for(1, 1_000_000_000), 1);
        assert!(capacity_for(10, 250_000).is_power_of_two());
    }

    #[test]
    fn push_then_read_all() {
        let mut w = writer(8);
        for i in 0..5 {
            w.push(&sample(i));
        }
        let r = reader_of(&w);
        let mut out = Vec::new();
        let range = r.read_range(0, 100, &mut out);
        assert_eq!(range.first, 0);
        assert_eq!(range.next, 5);
        assert_eq!(range.dropped, 0);
        assert_eq!(out.len(), 5);
        assert_eq!(out[4], sample(4));
    }

    #[test]
    fn ring_overwrite_reports_dropped() {
        let mut w = writer(8);
        for i in 0..20 {
            w.push(&sample(i));
        }
        let r = reader_of(&w);
        let mut out = Vec::new();
        // Ask from 0: the last 8 live, minus the one the mid-push guard
        // conservatively discards (the reader cannot tell an idle writer
        // from one mid-push into the oldest slot).
        let range = r.read_range(0, 100, &mut out);
        assert_eq!(range.first, 13);
        assert_eq!(range.next, 20);
        assert_eq!(range.dropped, 13);
        assert_eq!(out.len(), 7);
        assert_eq!(out[0], sample(13));
        // Cursor continuation: nothing new.
        let range = r.read_range(range.next, 100, &mut out);
        assert_eq!(out.len(), 0);
        assert_eq!(range.next, 20);
        assert_eq!(range.dropped, 0);
    }

    #[test]
    fn max_caps_batch() {
        let mut w = writer(16);
        for i in 0..10 {
            w.push(&sample(i));
        }
        let r = reader_of(&w);
        let mut out = Vec::new();
        let range = r.read_range(0, 4, &mut out);
        assert_eq!(out.len(), 4);
        assert_eq!(range.next, 4);
        let range = r.read_range(range.next, 4, &mut out);
        assert_eq!(out[0], sample(4));
        assert_eq!(range.next, 8);
    }

    #[test]
    fn reader_rejects_bad_header() {
        let m = Arc::new(Mapping::in_memory(segment_size(8)));
        match TraceReader::new(m) {
            Err(e) => assert_eq!(e, TraceReadError::MagicMismatch),
            Ok(_) => panic!("zeroed segment must be rejected"),
        }
    }

    /// Writer hammers the ring while a reader chases it; every sample that
    /// survives validation must be internally consistent
    /// (`axes[0].act_pos == cycle as f64`).
    #[test]
    fn concurrent_reader_never_sees_torn_samples() {
        use std::sync::atomic::AtomicBool;
        use std::sync::Arc;

        let w = writer(64);
        let r = reader_of(&w);
        let stop = Arc::new(AtomicBool::new(false));

        let writer_thread = {
            let stop = Arc::clone(&stop);
            let mut w = w;
            std::thread::spawn(move || {
                for i in 0..300_000u64 {
                    w.push(&sample(i));
                }
                stop.store(true, Ordering::Release);
            })
        };

        let mut out = Vec::new();
        let mut cursor = 0u64;
        let mut seen = 0u64;
        while !stop.load(Ordering::Acquire) {
            let range = r.read_range(cursor, 512, &mut out);
            cursor = range.next;
            for (k, s) in out.iter().enumerate() {
                assert_eq!(s.cycle, range.first + k as u64, "index/content skew");
                assert_eq!(s.axes[0].act_pos, s.cycle as f64, "torn sample");
                seen += 1;
            }
        }
        writer_thread.join().unwrap();
        assert!(seen > 0);
    }
}
