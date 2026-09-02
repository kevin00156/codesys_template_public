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
