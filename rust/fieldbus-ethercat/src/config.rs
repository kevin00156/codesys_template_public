//! Adapter configuration — plain data, no bus types, shared by the real
//! Linux backend, the non-Linux stub and the bring-up example.

use std::time::Duration;

/// One served axis: which subdevice on the chain it is and how drive
/// increments map to user units.
#[derive(Clone, Copy, Debug)]
pub struct AxisMapping {
    /// Position of the drive on the EtherCAT chain (0-based, couplers and
    /// I/O blocks count too).
    pub subdevice: usize,
    /// Drive increments per user unit (position); velocity uses the same
    /// factor per second. The adapter owns all scaling (README ADR-1).
    pub scale: f64,
}

#[derive(Clone, Debug)]
pub struct EcatConfig {
    /// NIC bound to this MainDevice instance. A future dual-master setup is
    /// two `EthercatBackend`s with two configs — nothing here is global.
    pub ifname: String,
    /// PDO cycle time (also the DC SYNC0 period when `dc` is on).
    pub cycle: Duration,
    pub axes: Vec<AxisMapping>,
    /// Configure distributed clocks (SYNC0) on the drives.
    pub dc: bool,
    /// Write our CSP PDO mapping (pdo.rs) into 0x1600/0x1A00 during PREOP.
    /// Turn off for drives with fixed PDO layouts that already match.
    pub configure_pdo: bool,
    /// Map 0x60FD digital inputs into the TxPDO (limit/home switches).
    pub map_digital_inputs: bool,
}

impl EcatConfig {
    pub fn new(ifname: impl Into<String>, axes: usize) -> EcatConfig {
        EcatConfig {
            ifname: ifname.into(),
            cycle: Duration::from_millis(1),
            axes: (0..axes)
                .map(|i| AxisMapping {
                    subdevice: i,
                    scale: 10_000.0,
                })
                .collect(),
            dc: true,
            configure_pdo: true,
            map_digital_inputs: true,
        }
    }
}
