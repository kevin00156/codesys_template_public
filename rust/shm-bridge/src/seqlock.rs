//! The seqlock protocol from `backend/internal/shm/{writer,reader}.go`,
//! generalised over the two segment types.
//!
//! Wire protocol (unchanged, shared with Go and the IEC runtime):
//! the writer makes `Header.Seq` odd, mutates the payload, then makes it even
//! again; a reader that sees an odd seq, or different seqs before and after
//! its copy, retries. The daemon is the *writer* of `plc_data` and the
//! *reader* of `plc_cmd`.
//!
//! Memory model: unlike Go (whose bulk struct copy is racy under C11 rules
//! but tolerated by its runtime and x86-TSO), this implementation is fully
//! defined under the Rust/C11 model: the payload is copied as relaxed
//! per-`u64` atomic accesses and the seq transitions carry acquire/release
//! fences (the fence-based seqlock from Boehm, "Can seqlocks get along with
//! programming language memory models?"). On x86-64 all of it compiles to
//! plain MOVs plus compiler barriers — zero runtime cost, same bytes on the
//! wire.

use core::mem::size_of;
use core::sync::atomic::{fence, AtomicU32, AtomicU64, Ordering};

use crate::layout::{Header, Segment};
use crate::mapping::Mapping;

/// Mirror of Go's `seqlockMaxRetries` and the IEC reader's `FOR i := 0 TO 99`.
pub const SEQLOCK_MAX_RETRIES: usize = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadError {
    /// Writer kept us spinning (`ErrSeqlockBusy`).
    Busy,
    /// Wrong segment or layout (`ErrMagicMismatch`).
    MagicMismatch,
    /// Rebuild sides with matching layout (`ErrVersionMismatch`).
    VersionMismatch,
}

impl core::fmt::Display for ReadError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Busy => write!(f, "seqlock: writer kept us spinning"),
            Self::MagicMismatch => write!(f, "magic mismatch: wrong segment or layout"),
            Self::VersionMismatch => {
                write!(f, "version mismatch: rebuild peers with matching layout")
            }
        }
    }
}

impl std::error::Error for ReadError {}

const SEQ_OFFSET: usize = core::mem::offset_of!(Header, seq);
/// Index of the u64 word that contains `Header.Seq` (+ its padding). The
/// bulk copy loops skip it: the seq word is owned by the AtomicU32 below,
/// and mixing atomic access sizes on the same location is not defined.
const SEQ_WORD: usize = SEQ_OFFSET / 8;

fn seq_atomic(m: &Mapping) -> &AtomicU32 {
    debug_assert!(m.len() >= size_of::<Header>());
    // Safety: offset 8 is in bounds, 4-aligned (mapping is 8-aligned), and
    // every access to this location goes through atomics.
    unsafe { AtomicU32::from_ptr(m.ptr().add(SEQ_OFFSET) as *mut u32) }
}

/// Publish `src` to the segment under the seqlock protocol — the daemon-side
/// counterpart of Go's `WritePlcCommand` (same protocol, opposite segment).
///
/// The writer — not the caller — owns `Header.Seq`; whatever seq the caller
/// left in `src` is ignored, and the payload copy never carries an even seq
/// while the payload is in flux (see writer.go's comment for the tear this
/// prevents).
pub fn publish<T: Segment>(m: &Mapping, src: &T) {
    assert!(m.len() >= size_of::<T>(), "segment smaller than {}", T::NAME);
    let seq = seq_atomic(m);

    let s = seq.load(Ordering::Relaxed);
    let odd = s.wrapping_add(1); // sole writer: s is even, s+1 is odd
    seq.store(odd, Ordering::Relaxed);
    fence(Ordering::Release); // begin-write: payload stores stay after the odd store

    let mut tmp = *src;
    tmp.header_mut().seq = odd;
    tmp.header_mut()._pad = 0;
    let words = size_of::<T>() / 8;
    let src_words = &tmp as *const T as *const u64;
    let dst = m.ptr() as *mut u64;
    for i in 0..words {
        if i == SEQ_WORD {
            continue; // seq word is owned by the AtomicU32
        }
        // Safety: in bounds (asserted above), 8-aligned, atomic.
        unsafe { AtomicU64::from_ptr(dst.add(i)).store(src_words.add(i).read(), Ordering::Relaxed) };
    }

    seq.store(odd.wrapping_add(1), Ordering::Release); // end-write: even == stable
}

/// Copy the segment into `dst` under the seqlock protocol — the daemon-side
/// counterpart of Go's `ReadPlcData` (same protocol, opposite segment).
///
/// On success `dst.header.magic/version` are guaranteed to match this
/// build's expectations, exactly like the Go reader.
pub fn snapshot<T: Segment>(m: &Mapping, dst: &mut T) -> Result<(), ReadError> {
    assert!(m.len() >= size_of::<T>(), "segment smaller than {}", T::NAME);
    let seq = seq_atomic(m);
    let words = size_of::<T>() / 8;

    for _ in 0..SEQLOCK_MAX_RETRIES {
        let s1 = seq.load(Ordering::Acquire);
        if s1 & 1 != 0 {
            continue;
        }
        let src = m.ptr() as *const u64;
        let dst_words = dst as *mut T as *mut u64;
        for i in 0..words {
            if i == SEQ_WORD {
                continue;
            }
            // Safety: in bounds (asserted above), 8-aligned, atomic.
            unsafe {
                dst_words
                    .add(i)
                    .write(AtomicU64::from_ptr(src.add(i) as *mut u64).load(Ordering::Relaxed))
            };
        }
        fence(Ordering::Acquire); // end-read: payload loads complete before the re-check
        let s2 = seq.load(Ordering::Relaxed);
        if s1 != s2 {
            continue;
        }
        dst.header_mut().seq = s1;
        dst.header_mut()._pad = 0;
        if dst.header().magic != T::MAGIC {
            return Err(ReadError::MagicMismatch);
        }
        if dst.header().version != T::VERSION {
            return Err(ReadError::VersionMismatch);
        }
        return Ok(());
    }
    Err(ReadError::Busy)
}
