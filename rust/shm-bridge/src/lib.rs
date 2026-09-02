//! shm-bridge: byte-identical Rust replica of the PLC⇄Go shared-memory
//! contract (`backend/internal/shm`), plus daemon-side segment ownership.
//!
//! * [`layout`] — the `PlcData`/`PlcCommand` structs, pinned by compile-time
//!   asserts and by golden-fixture parity tests against the Go encoder.
//! * [`seqlock`] — the tear-free publish/snapshot protocol.
//! * [`mapping`] — /dev/shm segment creation (Linux) or in-memory buffers.
//! * [`channel`] — `DataPublisher` / `CmdReader` with the IEC program's
//!   command-latching semantics.
//! * [`trace`] — the `plc_trace` broadcast ring (one fixed sample per cycle)
//!   behind the HMI's watch/trace panels.

pub mod channel;
pub mod layout;
pub mod mapping;
pub mod seqlock;
pub mod trace;

pub use channel::{CmdReader, DataPublisher};
pub use layout::*;
pub use mapping::Mapping;
pub use seqlock::{publish, reset, snapshot, ReadError, SEQLOCK_MAX_RETRIES};
pub use trace::{TraceAxisSample, TraceHeader, TraceReader, TraceSample, TraceWriter};

#[cfg(test)]
mod tests {
    //! Mirrors backend/internal/shm/seqlock_test.go (round-trip, rejects,
    //! writer seq ownership) and seqlock_concurrent_test.go (torn reads).

    use super::*;
    use core::sync::atomic::{AtomicU32, Ordering};

    fn data_mapping() -> Mapping {
        Mapping::in_memory(SIZE_PLC_DATA)
    }

    fn cmd_mapping() -> Mapping {
        Mapping::in_memory(SIZE_PLC_COMMAND)
    }

    /// Poke header fields straight into the raw segment (test-only writer,
    /// like the Go tests mutating the aliased struct).
    fn seed_header(m: &Mapping, magic: u32, version: u16, seq: u32, cycle: u64) {
        let mut d = PlcData::default();
        d.header.magic = magic;
        d.header.version = version;
        d.header.cycle = cycle;
        let bytes = layout::as_bytes(&d);
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), m.ptr(), bytes.len().min(m.len()));
            AtomicU32::from_ptr(m.ptr().add(8) as *mut u32).store(seq, Ordering::Release);
        }
    }

    #[test]
    fn snapshot_round_trip() {
        let m = data_mapping();
        seed_header(&m, PLC_DATA_MAGIC, PLC_DATA_VERSION, 2, 42);

        let mut dst = PlcData::default();
        snapshot(&m, &mut dst).expect("snapshot");
        assert_eq!(dst.header.cycle, 42);
        assert_eq!(dst.header.seq, 2);
    }

    #[test]
    fn snapshot_rejects_magic() {
        let m = data_mapping();
        seed_header(&m, 0xBADBAD, PLC_DATA_VERSION, 2, 0);
        let mut dst = PlcData::default();
        assert_eq!(snapshot(&m, &mut dst), Err(ReadError::MagicMismatch));
    }

    #[test]
    fn snapshot_rejects_version() {
        let m = data_mapping();
        seed_header(&m, PLC_DATA_MAGIC, PLC_DATA_VERSION + 99, 2, 0);
        let mut dst = PlcData::default();
        assert_eq!(snapshot(&m, &mut dst), Err(ReadError::VersionMismatch));
    }

    #[test]
    fn snapshot_rejects_busy() {
        let m = data_mapping();
        // odd forever -> writer perpetually in progress
        seed_header(&m, PLC_DATA_MAGIC, PLC_DATA_VERSION, 1, 0);
        let mut dst = PlcData::default();
        assert_eq!(snapshot(&m, &mut dst), Err(ReadError::Busy));
    }

    #[test]
    fn publish_round_trip_and_seq_ownership() {
        let m = cmd_mapping();

        let mut src = PlcCommand::default();
        src.header.magic = PLC_COMMAND_MAGIC;
        src.header.version = PLC_COMMAND_VERSION;
        src.header.seq = 999; // garbage: the writer owns seq and must ignore this
        src.header.cycle = 7;

        publish(&m, &src);

        let mut got = PlcCommand::default();
        snapshot(&m, &mut got).expect("seqlock never stabilised");
        assert_eq!(got.header.cycle, 7);
        assert_eq!(got.header.magic, PLC_COMMAND_MAGIC);
        assert_eq!(got.header.version, PLC_COMMAND_VERSION);
        // First write must land on the first even value, not anything
        // derived from src.header.seq (999).
        assert_eq!(got.header.seq, 2, "writer must ignore src seq");

        // Second write advances seq by exactly 2 and stays even.
        src.header.cycle = 8;
        publish(&m, &src);
        let mut got2 = PlcCommand::default();
        snapshot(&m, &mut got2).expect("seqlock never stabilised on 2nd write");
        assert_eq!(got2.header.seq, 4);
        assert_eq!(got2.header.cycle, 8);
    }

    /// Writer hammers the segment while a reader snapshots; two fields that
    /// must agree within one snapshot (flags == low 16 bits of cycle) expose
    /// torn reads. Mirrors seqlock_concurrent_test.go.
    #[test]
    fn concurrent_no_torn_reads() {
        use std::sync::atomic::AtomicBool;
        use std::sync::Arc;

        let m = Arc::new(cmd_mapping());
        let stop = Arc::new(AtomicBool::new(false));

        let writer = {
            let m = Arc::clone(&m);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                let mut src = PlcCommand::default();
                src.header.magic = PLC_COMMAND_MAGIC;
                src.header.version = PLC_COMMAND_VERSION;
                for i in 1u64..=500_000 {
                    src.header.flags = i as u16;
                    src.header.cycle = i;
                    publish(&*m, &src);
                }
                stop.store(true, Ordering::Release);
            })
        };

        let mut torn = 0u64;
        let mut reads = 0u64;
        while !stop.load(Ordering::Acquire) {
            let mut got = PlcCommand::default();
            if snapshot(&*m, &mut got).is_ok() {
                reads += 1;
                if got.header.flags != got.header.cycle as u16 {
                    torn += 1;
                }
            }
        }
        writer.join().unwrap();
        assert_eq!(torn, 0, "observed {torn} torn reads out of {reads}");
        assert!(reads > 0);
    }

    /// Reusing an existing segment across a daemon restart: reset() must
    /// leave it rejecting (magic 0) with an even seq that differs from the
    /// pre-reset value, and the next publish must continue cleanly.
    #[test]
    fn reset_invalidates_and_keeps_seq_monotonic() {
        let m = cmd_mapping();
        let mut src = PlcCommand::default();
        src.header.magic = PLC_COMMAND_MAGIC;
        src.header.version = PLC_COMMAND_VERSION;
        src.header.cycle = 7;
        src.machine.axes[0].control_flags = 0x30; // stale jog word
        publish(&m, &src);
        let before = unsafe { AtomicU32::from_ptr(m.ptr().add(8) as *mut u32) }.load(Ordering::Acquire);
        assert_eq!(before, 2);

        reset::<PlcCommand>(&m);
        let after = unsafe { AtomicU32::from_ptr(m.ptr().add(8) as *mut u32) }.load(Ordering::Acquire);
        assert_eq!(after % 2, 0, "reset must end on an even seq");
        assert_ne!(after, before, "seq must move so a racing reader retries");

        let mut got = PlcCommand::default();
        assert_eq!(snapshot(&m, &mut got), Err(ReadError::MagicMismatch));
        assert_eq!(got.machine.axes[0].control_flags, 0, "payload zeroed");

        // A stale odd seq (writer died mid-write) is repaired too.
        unsafe { AtomicU32::from_ptr(m.ptr().add(8) as *mut u32) }.store(9, Ordering::Release);
        reset::<PlcCommand>(&m);
        let repaired = unsafe { AtomicU32::from_ptr(m.ptr().add(8) as *mut u32) }.load(Ordering::Acquire);
        assert_eq!(repaired, 10);

        // Publishing afterwards continues from the reset seq.
        publish(&m, &src);
        snapshot(&m, &mut got).expect("valid after publish");
        assert_eq!(got.header.seq, 12);
        assert_eq!(got.header.cycle, 7);
    }

    #[test]
    fn cmd_reader_latches_on_cycle_change() {
        let m = cmd_mapping();
        // CmdReader borrows its own mapping; publish through a second view
        // of the same memory region is not possible with the safe API, so
        // drive it the way the daemon does: writer publishes into the same
        // Mapping shared by reference.
        let mut src = PlcCommand::default();
        src.header.magic = PLC_COMMAND_MAGIC;
        src.header.version = PLC_COMMAND_VERSION;

        // Before any writer: poll errors (magic mismatch on zeroed segment),
        // latch stays at the all-clear default.
        let mut reader = CmdReader::new(Mapping::in_memory(SIZE_PLC_COMMAND));
        assert!(reader.poll().is_err());
        assert_eq!(reader.current().machine.axes[0].control_flags, 0);

        // Same segment, real flow: new cycle -> latched once.
        src.header.cycle = 1;
        src.machine.axes[0].control_flags = 0b1;
        src.machine.axes[0].jog_vel = 12.5;
        publish(&m, &src);
        let mut reader = CmdReader::new(m);
        assert_eq!(reader.poll(), Ok(true), "first valid snapshot latches");
        assert_eq!(reader.current().machine.axes[0].control_flags, 0b1);
        assert_eq!(reader.current().machine.axes[0].jog_vel, 12.5);
        // Unchanged cycle -> no new latch, previous stays in force.
        assert_eq!(reader.poll(), Ok(false));
        assert_eq!(reader.current().machine.axes[0].control_flags, 0b1);
    }
}
