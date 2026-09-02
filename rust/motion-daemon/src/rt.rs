//! Linux RT plumbing + shutdown signal, cfg-isolated like Go's
//! `mapping_linux.go` / `mapping_other.go` split so the crate builds on the
//! Windows dev box.
//!
//! The sim backend runs on a normal thread (the spec exempts it from RT);
//! `lock_memory` / `set_fifo` are wired up by the EtherCAT cycle thread in
//! Phase 3.

use std::sync::atomic::{AtomicBool, Ordering};

/// Cleared by the first SIGINT/SIGTERM: the cycle loop leaves normal
/// operation and runs the controlled stop (EMS ramp, then drive disable —
/// see `engine::Engine::shutdown_step`).
pub static RUNNING: AtomicBool = AtomicBool::new(true);

/// Set by a *second* SIGINT/SIGTERM while the controlled stop is running:
/// the loop skips straight to `bus.stop()`. The operator asked twice, and
/// systemd sends SIGKILL after `TimeoutStopSec` regardless — better to cut
/// power under our control than to be killed mid-exchange.
pub static FORCE_QUIT: AtomicBool = AtomicBool::new(false);

pub fn running() -> bool {
    RUNNING.load(Ordering::Relaxed)
}

pub fn force_quit() -> bool {
    FORCE_QUIT.load(Ordering::Relaxed)
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{FORCE_QUIT, RUNNING};
    use std::io;
    use std::sync::atomic::Ordering;

    extern "C" fn on_signal(_sig: libc::c_int) {
        // First signal: stop running. Second: force. `swap` is one atomic
        // RMW — still async-signal-safe.
        if !RUNNING.swap(false, Ordering::Relaxed) {
            FORCE_QUIT.store(true, Ordering::Relaxed);
        }
    }

    /// SIGINT/SIGTERM flip [`super::RUNNING`] (first) and
    /// [`super::FORCE_QUIT`] (second); the cycle loop keeps exchanging
    /// through the controlled stop and only then releases the bus.
    pub fn install_shutdown_signals() {
        // Safety: on_signal is async-signal-safe (one atomic store).
        unsafe {
            libc::signal(
                libc::SIGINT,
                on_signal as extern "C" fn(libc::c_int) as *const () as libc::sighandler_t,
            );
            libc::signal(
                libc::SIGTERM,
                on_signal as extern "C" fn(libc::c_int) as *const () as libc::sighandler_t,
            );
        }
    }

    /// mlockall — no page faults on the RT path (Phase 3 EtherCAT thread).
    #[allow(dead_code)]
    pub fn lock_memory() -> io::Result<()> {
        // Safety: plain libc call, no pointers involved.
        if unsafe { libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// SCHED_FIFO for the calling thread (Phase 3 EtherCAT cycle thread).
    #[allow(dead_code)]
    pub fn set_fifo(priority: i32) -> io::Result<()> {
        // zeroed(), not a field literal: musl's sched_param carries extra
        // sched_ss_* (sporadic server) fields that glibc's does not, so a
        // field literal fails to compile against musl (the real IPC binary is
        // built static-musl for glibc-independence).
        // Safety: sched_param is a plain integer POSIX struct; all-zero is valid.
        let mut param: libc::sched_param = unsafe { std::mem::zeroed() };
        param.sched_priority = priority;
        // Safety: param outlives the call.
        if unsafe { libc::sched_setscheduler(0, libc::SCHED_FIFO, &param) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use std::io;

    /// Windows/macOS dev box: Ctrl-C terminates the process the default way;
    /// the daemon cannot mount /dev/shm there anyway.
    pub fn install_shutdown_signals() {}

    #[allow(dead_code)]
    pub fn lock_memory() -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "linux only"))
    }

    #[allow(dead_code)]
    pub fn set_fifo(_priority: i32) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "linux only"))
    }
}

pub use imp::*;
