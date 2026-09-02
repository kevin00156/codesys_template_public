//! A view over a shared-memory segment — the counterpart of Go's
//! `shm.Mapping` (`mapping.go` / `mapping_linux.go` / `mapping_other.go`).
//!
//! On Linux, [`Mapping::create`] owns a `/dev/shm/<name>` segment (the daemon
//! is the segment *creator*, taking over CODESYS's `SysSharedMemoryCreate`
//! role — see `PRG_ShmPublisher.st`). On other platforms it returns an error
//! so everything still compiles and pure-logic tests run on the dev box;
//! tests construct in-memory mappings with [`Mapping::in_memory`].

#[cfg(not(target_os = "linux"))]
use std::io;

pub struct Mapping {
    ptr: *mut u8,
    len: usize,
    /// Keeps the memory alive; read only by the Linux Drop impl.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    backing: Backing,
}

enum Backing {
    /// Test / loopback mapping; the box keeps the memory alive and 8-aligned.
    InMemory(#[allow(dead_code)] Box<[u64]>),
    /// mmap of /dev/shm/<name>; munmapped on drop (linux only).
    #[cfg(target_os = "linux")]
    Shm,
}

// Safety: all concurrent access goes through the seqlock's atomic operations
// (see seqlock.rs); the raw pointer itself is stable for the Mapping's life.
unsafe impl Send for Mapping {}
unsafe impl Sync for Mapping {}

impl Mapping {
    /// An anonymous in-memory mapping (zeroed, 8-byte aligned). Used by unit
    /// tests and by a possible future loopback mode; never touches /dev/shm.
    pub fn in_memory(size: usize) -> Mapping {
        let words = size.div_ceil(8);
        let mut buf = vec![0u64; words].into_boxed_slice();
        let ptr = buf.as_mut_ptr() as *mut u8;
        Mapping {
            ptr,
            len: words * 8,
            backing: Backing::InMemory(buf),
        }
    }

    pub fn ptr(&self) -> *mut u8 {
        self.ptr
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{Backing, Mapping};
    use std::ffi::CString;
    use std::io;

    impl Mapping {
        /// Create `/dev/shm/<name>` fresh: unlink any stale segment, create,
        /// force `mode` (bypassing umask), size and map it.
        ///
        /// Recreating mirrors CODESYS behaviour — the runtime re-creates the
        /// segments on every restart, which is why `plc_bridge.service.unit`
        /// re-runs its ExecStartPre chmod on every start. `mode` should
        /// already grant the bridge user access (0644 for plc_data, 0666 for
        /// plc_cmd) so that chmod becomes redundant but stays harmless.
        pub fn create(name: &str, size: usize, mode: u32) -> io::Result<Mapping> {
            assert!(size > 0);
            let c_name = CString::new(format!("/{name}"))
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "name contains NUL"))?;

            // Stale segments carry old layouts and old command words; start
            // from a zeroed page like a fresh CODESYS boot does.
            // Safety: FFI calls with a valid, NUL-terminated name.
            unsafe {
                libc::shm_unlink(c_name.as_ptr()); // ENOENT is fine
                let fd = libc::shm_open(
                    c_name.as_ptr(),
                    libc::O_CREAT | libc::O_EXCL | libc::O_RDWR,
                    0o600,
                );
                if fd < 0 {
                    return Err(io::Error::last_os_error());
                }
                let guard = FdGuard(fd);
                if libc::fchmod(fd, mode as libc::mode_t) != 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::ftruncate(fd, size as libc::off_t) != 0 {
                    return Err(io::Error::last_os_error());
                }
                let ptr = libc::mmap(
                    core::ptr::null_mut(),
                    size,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    fd,
                    0,
                );
                if ptr == libc::MAP_FAILED {
                    return Err(io::Error::last_os_error());
                }
                drop(guard); // mapping survives fd close
                Ok(Mapping {
                    ptr: ptr as *mut u8,
                    len: size,
                    backing: Backing::Shm,
                })
            }
        }

        /// Open `/dev/shm/<name>` if it exists, otherwise create it; force
        /// `mode`, grow it to at least `size` and map the first `size` bytes.
        ///
        /// Unlike [`Mapping::create`] this **keeps the segment's inode**. The
        /// Go bridge opens `plc_data`/`plc_cmd` once at start-up and never
        /// re-opens them, so an unlink-and-recreate on daemon restart would
        /// leave the bridge reading and writing an orphaned inode forever
        /// (frozen data, commands that never arrive). Reusing the inode keeps
        /// the bridge's mappings live across daemon restarts.
        ///
        /// The contents are whatever the previous owner left. Callers that
        /// need a clean "no writer yet" state must run
        /// `seqlock::reset::<T>()` on the result — it zeroes the payload under
        /// the seqlock protocol so a concurrent reader never latches a torn
        /// old/zero mix. The file is only ever grown, never shrunk: shrinking
        /// a file another process has mapped makes its accesses SIGBUS.
        pub fn open_or_create(name: &str, size: usize, mode: u32) -> io::Result<Mapping> {
            assert!(size > 0);
            let c_name = CString::new(format!("/{name}"))
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "name contains NUL"))?;

            // Safety: FFI calls with a valid, NUL-terminated name; `st` is a
            // plain C struct for which all-zero is a valid initial value.
            unsafe {
                let fd = libc::shm_open(c_name.as_ptr(), libc::O_CREAT | libc::O_RDWR, 0o600);
                if fd < 0 {
                    return Err(io::Error::last_os_error());
                }
                let guard = FdGuard(fd);
                if libc::fchmod(fd, mode as libc::mode_t) != 0 {
                    return Err(io::Error::last_os_error());
                }
                let mut st: libc::stat = core::mem::zeroed();
                if libc::fstat(fd, &mut st) != 0 {
                    return Err(io::Error::last_os_error());
                }
                if (st.st_size as u64) < size as u64 && libc::ftruncate(fd, size as libc::off_t) != 0 {
                    return Err(io::Error::last_os_error());
                }
                let ptr = libc::mmap(
                    core::ptr::null_mut(),
                    size,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    fd,
                    0,
                );
                if ptr == libc::MAP_FAILED {
                    return Err(io::Error::last_os_error());
                }
                drop(guard);
                Ok(Mapping {
                    ptr: ptr as *mut u8,
                    len: size,
                    backing: Backing::Shm,
                })
            }
        }
    }

    struct FdGuard(libc::c_int);
    impl Drop for FdGuard {
        fn drop(&mut self) {
            // Safety: fd is owned by this guard.
            unsafe { libc::close(self.0) };
        }
    }

    impl Drop for Mapping {
        fn drop(&mut self) {
            if let Backing::Shm = self.backing {
                // Safety: ptr/len came from a successful mmap. The segment
                // file itself is intentionally left in /dev/shm so late
                // readers keep the last published data, like CODESYS did.
                unsafe { libc::munmap(self.ptr as *mut libc::c_void, self.len) };
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
impl Mapping {
    /// /dev/shm exists only on Linux; fail at runtime but compile everywhere
    /// (mirrors Go's `mapping_other.go`).
    pub fn create(_name: &str, _size: usize, _mode: u32) -> io::Result<Mapping> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "shm: /dev/shm segments are only supported on linux, not {}",
                std::env::consts::OS
            ),
        ))
    }

    /// See the Linux implementation; unsupported elsewhere.
    pub fn open_or_create(name: &str, size: usize, mode: u32) -> io::Result<Mapping> {
        Self::create(name, size, mode)
    }
}

/// Real `/dev/shm` behaviour — the inode contract that `open_or_create` vs
/// `create` exists for. Only runnable on Linux (WSL counts); each test uses
/// its own pid-tagged segment name and unlinks it on exit, panic included.
#[cfg(all(test, target_os = "linux"))]
mod linux_tests {
    use super::Mapping;
    use crate::layout::{PlcCommand, PLC_COMMAND_MAGIC, PLC_COMMAND_VERSION, SIZE_PLC_COMMAND, SIZE_PLC_DATA};
    use crate::seqlock::{self, ReadError};
    use core::sync::atomic::{AtomicU64, Ordering};
    use std::ffi::CString;
    use std::os::unix::fs::MetadataExt;

    /// A uniquely named segment that is `shm_unlink`ed when dropped — also
    /// on panic — so a failed test never litters `/dev/shm`. The tag keeps
    /// tests of the same process (cargo runs them in parallel) apart.
    struct SegmentGuard(String);

    impl SegmentGuard {
        fn new(tag: &str) -> SegmentGuard {
            SegmentGuard(format!("shm_bridge_test_{tag}_{}", std::process::id()))
        }

        fn name(&self) -> &str {
            &self.0
        }

        fn metadata(&self) -> std::fs::Metadata {
            std::fs::metadata(format!("/dev/shm/{}", self.0)).expect("segment file exists")
        }

        fn ino(&self) -> u64 {
            self.metadata().ino()
        }
    }

    impl Drop for SegmentGuard {
        fn drop(&mut self) {
            let c = CString::new(format!("/{}", self.0)).expect("no NUL in test name");
            // Safety: FFI call with a valid, NUL-terminated name; ENOENT
            // (segment never created) is fine.
            unsafe { libc::shm_unlink(c.as_ptr()) };
        }
    }

    /// `/dev/shm` can be missing or read-only in minimal containers; the
    /// tests skip (not fail) there so `cargo test` stays meaningful.
    fn dev_shm_writable() -> bool {
        let probe = format!("/dev/shm/shm_bridge_probe_{}", std::process::id());
        match std::fs::File::create(&probe) {
            Ok(_) => {
                let _ = std::fs::remove_file(&probe);
                true
            }
            Err(e) => {
                eprintln!("skipping: /dev/shm is not writable ({e})");
                false
            }
        }
    }

    /// Atomic view of the first payload word (offset 24, just past the
    /// header) of a `PlcCommand`-sized mapping.
    fn payload_word(m: &Mapping) -> &AtomicU64 {
        assert!(m.len() >= 32);
        // Safety: in bounds (asserted), mmap is page-aligned so offset 24 is
        // 8-aligned; every access to this word is atomic.
        unsafe { AtomicU64::from_ptr(m.ptr().add(24) as *mut u64) }
    }

    fn valid_cmd(cycle: u64) -> PlcCommand {
        let mut c = PlcCommand::default();
        c.header.magic = PLC_COMMAND_MAGIC;
        c.header.version = PLC_COMMAND_VERSION;
        c.header.cycle = cycle;
        c.machine.axes[0].control_flags = 0x30; // a jog word, visibly non-zero
        c
    }

    /// The daemon-restart case: a second `open_or_create` must attach to
    /// the *same* inode so a peer that mapped the segment earlier (the Go
    /// bridge) keeps seeing new data through its old mapping.
    #[test]
    fn open_or_create_reuses_inode_and_aliases_memory() {
        if !dev_shm_writable() {
            return;
        }
        let g = SegmentGuard::new("alias");
        let a = Mapping::open_or_create(g.name(), SIZE_PLC_COMMAND, 0o666).expect("first open");
        let ino = g.ino();
        assert_eq!(g.metadata().mode() & 0o777, 0o666, "mode forced past umask");
        assert_eq!(g.metadata().len(), SIZE_PLC_COMMAND as u64);

        let b = Mapping::open_or_create(g.name(), SIZE_PLC_COMMAND, 0o666).expect("second open");
        assert_eq!(g.ino(), ino, "reopen must keep the inode");
        assert_eq!(a.len(), b.len());

        payload_word(&a).store(0xDEAD_BEEF_CAFE_F00D, Ordering::Release);
        assert_eq!(
            payload_word(&b).load(Ordering::Acquire),
            0xDEAD_BEEF_CAFE_F00D,
            "both mappings must alias the same physical pages"
        );
    }

    /// `seqlock::reset` through a fresh mapping (what the restarted daemon
    /// does) must invalidate what an older mapping of the same inode sees:
    /// magic 0, payload zeroed, inode untouched.
    #[test]
    fn reset_through_second_mapping_invalidates_first() {
        if !dev_shm_writable() {
            return;
        }
        let g = SegmentGuard::new("reset");
        let first = Mapping::open_or_create(g.name(), SIZE_PLC_COMMAND, 0o666).expect("first open");
        let ino = g.ino();
        seqlock::publish(&first, &valid_cmd(3));
        let mut got = PlcCommand::default();
        seqlock::snapshot(&first, &mut got).expect("valid before reset");
        assert_eq!(got.machine.axes[0].control_flags, 0x30);

        let second = Mapping::open_or_create(g.name(), SIZE_PLC_COMMAND, 0o666).expect("second open");
        seqlock::reset::<PlcCommand>(&second);

        assert_eq!(seqlock::snapshot(&first, &mut got), Err(ReadError::MagicMismatch));
        assert_eq!(got.machine.axes[0].control_flags, 0, "payload zeroed through the other mapping");
        assert_eq!(g.ino(), ino, "reset must not touch the file identity");
    }

    /// `create` (unlink + `O_EXCL`) deliberately yields a *new* inode: a
    /// peer still mapping the old one is orphaned and never sees the new
    /// writer. That is fine for `plc_trace`, whose Go reader watches the
    /// inode and remaps, and exactly why `plc_data`/`plc_cmd` — which the Go
    /// bridge maps once and never re-opens — must use `open_or_create`.
    #[test]
    fn create_replaces_inode_and_orphans_old_mapping() {
        if !dev_shm_writable() {
            return;
        }
        let g = SegmentGuard::new("recreate");
        let old = Mapping::open_or_create(g.name(), SIZE_PLC_COMMAND, 0o666).expect("open");
        let ino_old = g.ino();

        let fresh = Mapping::create(g.name(), SIZE_PLC_COMMAND, 0o644).expect("create");
        assert_ne!(g.ino(), ino_old, "create must replace the inode");
        assert_eq!(g.metadata().mode() & 0o777, 0o644);

        // The old mapping still works but is now an orphan: nothing written
        // through it reaches the segment a fresh opener sees.
        payload_word(&old).store(0x1234_5678_9ABC_DEF0, Ordering::Release);
        assert_eq!(payload_word(&fresh).load(Ordering::Acquire), 0, "new inode starts zeroed");
        let reopened = Mapping::open_or_create(g.name(), SIZE_PLC_COMMAND, 0o644).expect("reopen");
        assert_eq!(payload_word(&reopened).load(Ordering::Acquire), 0, "reopen attaches to the new inode");
    }

    /// The file only ever grows: a bigger `size` extends it (so an older,
    /// smaller layout can be upgraded in place), a smaller `size` maps just
    /// that prefix and leaves the file alone — shrinking would SIGBUS any
    /// peer that mapped the larger size.
    #[test]
    fn open_or_create_grows_but_never_shrinks() {
        if !dev_shm_writable() {
            return;
        }
        let g = SegmentGuard::new("grow");
        let small = Mapping::open_or_create(g.name(), SIZE_PLC_COMMAND, 0o666).expect("open small");
        assert_eq!(g.metadata().len(), SIZE_PLC_COMMAND as u64);
        assert_eq!(small.len(), SIZE_PLC_COMMAND);

        let ino = g.ino();
        let big = Mapping::open_or_create(g.name(), SIZE_PLC_DATA, 0o666).expect("open bigger");
        assert_eq!(g.metadata().len(), SIZE_PLC_DATA as u64, "smaller file is grown");
        assert_eq!(big.len(), SIZE_PLC_DATA);
        assert_eq!(g.ino(), ino, "growing keeps the inode");
        drop(small); // unmapping one view must not affect the file

        let again = Mapping::open_or_create(g.name(), SIZE_PLC_COMMAND, 0o666).expect("open smaller");
        assert_eq!(again.len(), SIZE_PLC_COMMAND, "maps only what was asked for");
        assert_eq!(g.metadata().len(), SIZE_PLC_DATA as u64, "larger file is never shrunk");
        // The tail past `again`'s view is still live for `big`.
        // Safety: SIZE_PLC_DATA - 8 is in bounds of `big` and 8-aligned.
        let tail = unsafe { AtomicU64::from_ptr(big.ptr().add(SIZE_PLC_DATA - 8) as *mut u64) };
        tail.store(7, Ordering::Release);
        assert_eq!(tail.load(Ordering::Acquire), 7);
    }
}
