//! EtherCAT adapter: ethercrab `MainDevice` behind the `fieldbus-api` seam.
//!
//! Hard rules carried over from the kickoff spec:
//! * **ethercrab types never appear in this crate's public API** — the IgH
//!   fallback stays possible only if the seam stays clean. Its DS402 helpers
//!   are not used for sequencing; our own [`cia402`] owns the
//!   statusword/controlword layer (README ADR-4).
//! * PDI guard hold times minimal (0.6+ uses a spinlock): the cyclic path
//!   copies each axis' few PDO bytes in/out and releases immediately.
//! * No single-master assumptions: one `EthercatBackend` per NIC; dual
//!   master = two instances with two [`EcatConfig`]s.
//! * RT discipline in the cycle path: zero heap allocation per cycle;
//!   SCHED_FIFO/mlockall are applied by the daemon before entering the loop.
//!
//! Platform split (README ADR-9/-10): the real backend is Linux-only (raw
//! sockets); other platforms get a stub that fails at `start()` but compiles,
//! so `cargo build/test --workspace` stays green on the Windows dev box while
//! the pure logic ([`cia402`], [`pdo`]) tests everywhere.

pub mod cia402;
pub mod config;
pub mod pdo;

#[cfg(target_os = "linux")]
mod backend;
#[cfg(target_os = "linux")]
pub use backend::{EthercatAcyclic, EthercatBackend};

#[cfg(not(target_os = "linux"))]
mod backend_stub;
#[cfg(not(target_os = "linux"))]
pub use backend_stub::{EthercatAcyclic, EthercatBackend};

pub use config::{AxisMapping, EcatConfig};
