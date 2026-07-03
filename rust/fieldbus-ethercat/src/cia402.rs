//! CiA 402 drive-profile power state machine — statusword decode +
//! controlword sequencing. **Our own implementation** (kickoff §0: ethercrab's
//! DS402 helpers may at most be referenced internally, never trusted with the
//! sequencing), pure logic with no bus types, so it unit-tests on every
//! platform and could graduate to a shared crate if a 402-over-Modbus servo
//! ever shows up (README ADR-4).
//!
//! Scope: the power chain (Switch On Disabled → Ready → Switched On →
//! Operation Enabled) plus fault reset edges. Homing-mode handshakes come
//! later with the drive domain knowledge (`MC_WriteHomingParameters`).

use fieldbus_api::DriveStatus;

/// CiA 402 state, decoded from the statusword's canonical mask table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    NotReadyToSwitchOn,
    SwitchOnDisabled,
    ReadyToSwitchOn,
    SwitchedOn,
    OperationEnabled,
    QuickStopActive,
    FaultReactionActive,
    Fault,
}

/// Canonical statusword state decode (CiA DSP402 table).
pub fn decode_statusword(sw: u16) -> State {
    // Order matters: the wider masks (0x6F) are checked before the 0x4F
    // fallbacks that would shadow them.
    if sw & 0x6F == 0x27 {
        State::OperationEnabled
    } else if sw & 0x6F == 0x23 {
        State::SwitchedOn
    } else if sw & 0x6F == 0x21 {
        State::ReadyToSwitchOn
    } else if sw & 0x6F == 0x07 {
        State::QuickStopActive
    } else if sw & 0x4F == 0x40 {
        State::SwitchOnDisabled
    } else if sw & 0x4F == 0x0F {
        State::FaultReactionActive
    } else if sw & 0x4F == 0x08 {
        State::Fault
    } else {
        State::NotReadyToSwitchOn
    }
}

/// Statusword bit 3: fault (redundant with the state decode, kept for
/// diagnostics).
pub const SW_FAULT_BIT: u16 = 1 << 3;

// Controlword command patterns (bits 0..3 + bit 7).
pub const CW_DISABLE_VOLTAGE: u16 = 0x0000;
pub const CW_SHUTDOWN: u16 = 0x0006;
pub const CW_SWITCH_ON: u16 = 0x0007;
pub const CW_ENABLE_OPERATION: u16 = 0x000F;
pub const CW_FAULT_RESET: u16 = 0x0080; // bit 7 rising edge

/// Per-axis 402 sequencer. Feed it the drive's statusword and the normalized
/// demand (`AxisOut::enable` / `fault_reset`) every cycle; it emits the
/// controlword. Stateless except for the fault-reset edge generator.
#[derive(Clone, Copy, Debug, Default)]
pub struct Cia402 {
    /// Fault-reset edge latch: bit 7 must *rise* to clear a fault, so we
    /// emit it for exactly one cycle per request and re-arm when the
    /// level-held request drops (mirrors how AxisOut::fault_reset is held).
    reset_fired: bool,
}

impl Cia402 {
    /// One cycle: statusword in, controlword out.
    pub fn controlword(&mut self, enable: bool, fault_reset: bool, sw: u16) -> u16 {
        let state = decode_statusword(sw);

        if !fault_reset {
            self.reset_fired = false;
        }

        match state {
            State::Fault => {
                if fault_reset && !self.reset_fired {
                    self.reset_fired = true;
                    return CW_FAULT_RESET;
                }
                CW_DISABLE_VOLTAGE
            }
            // Let the drive finish its fault reaction ramp untouched.
            State::FaultReactionActive => CW_DISABLE_VOLTAGE,
            State::NotReadyToSwitchOn => CW_SHUTDOWN, // drive still booting
            State::SwitchOnDisabled => CW_SHUTDOWN,
            State::ReadyToSwitchOn => {
                if enable {
                    CW_SWITCH_ON
                } else {
                    CW_SHUTDOWN // parked here when not enabled
                }
            }
            State::SwitchedOn => {
                if enable {
                    CW_ENABLE_OPERATION
                } else {
                    CW_SHUTDOWN
                }
            }
            State::OperationEnabled => {
                if enable {
                    CW_ENABLE_OPERATION
                } else {
                    CW_SHUTDOWN // transition 8: ramp down to ReadyToSwitchOn
                }
            }
            State::QuickStopActive => {
                if enable {
                    CW_ENABLE_OPERATION // transition 16 (if the drive allows)
                } else {
                    CW_SHUTDOWN
                }
            }
        }
    }

    /// Map the 402 state onto the normalized [`DriveStatus`] vocabulary.
    pub fn drive_status(enable_requested: bool, sw: u16) -> DriveStatus {
        match decode_statusword(sw) {
            State::OperationEnabled => DriveStatus::Enabled,
            State::QuickStopActive => DriveStatus::QuickStop,
            State::Fault | State::FaultReactionActive => DriveStatus::Fault,
            State::NotReadyToSwitchOn
            | State::SwitchOnDisabled
            | State::ReadyToSwitchOn
            | State::SwitchedOn => {
                if enable_requested {
                    DriveStatus::Enabling
                } else {
                    DriveStatus::Disabled
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Representative statuswords (low byte per the mask table; high bits set
    // to prove masking ignores them).
    const SW_SWITCH_ON_DISABLED: u16 = 0x0640; // voltage bit + misc
    const SW_READY_TO_SWITCH_ON: u16 = 0x0231;
    const SW_SWITCHED_ON: u16 = 0x0233;
    const SW_OPERATION_ENABLED: u16 = 0x0237;
    const SW_QUICK_STOP_ACTIVE: u16 = 0x0207;
    const SW_FAULT: u16 = 0x0208;
    const SW_FAULT_REACTION: u16 = 0x020F;
    const SW_NOT_READY: u16 = 0x0200;

    #[test]
    fn decode_matches_mask_table() {
        assert_eq!(decode_statusword(SW_SWITCH_ON_DISABLED), State::SwitchOnDisabled);
        assert_eq!(decode_statusword(SW_READY_TO_SWITCH_ON), State::ReadyToSwitchOn);
        assert_eq!(decode_statusword(SW_SWITCHED_ON), State::SwitchedOn);
        assert_eq!(decode_statusword(SW_OPERATION_ENABLED), State::OperationEnabled);
        assert_eq!(decode_statusword(SW_QUICK_STOP_ACTIVE), State::QuickStopActive);
        assert_eq!(decode_statusword(SW_FAULT), State::Fault);
        assert_eq!(decode_statusword(SW_FAULT_REACTION), State::FaultReactionActive);
        assert_eq!(decode_statusword(SW_NOT_READY), State::NotReadyToSwitchOn);
    }

    /// The canonical power-up walk: each state must be answered with the
    /// controlword that requests the next transition.
    #[test]
    fn enable_sequence_walks_the_chain() {
        let mut seq = Cia402::default();
        assert_eq!(seq.controlword(true, false, SW_SWITCH_ON_DISABLED), CW_SHUTDOWN);
        assert_eq!(seq.controlword(true, false, SW_READY_TO_SWITCH_ON), CW_SWITCH_ON);
        assert_eq!(seq.controlword(true, false, SW_SWITCHED_ON), CW_ENABLE_OPERATION);
        assert_eq!(seq.controlword(true, false, SW_OPERATION_ENABLED), CW_ENABLE_OPERATION);
    }

    #[test]
    fn disable_ramps_down_and_parks_ready() {
        let mut seq = Cia402::default();
        assert_eq!(seq.controlword(false, false, SW_OPERATION_ENABLED), CW_SHUTDOWN);
        assert_eq!(seq.controlword(false, false, SW_SWITCHED_ON), CW_SHUTDOWN);
        assert_eq!(seq.controlword(false, false, SW_READY_TO_SWITCH_ON), CW_SHUTDOWN);
    }

    /// Bit 7 must rise exactly once per held request, and re-arm only after
    /// the request is released.
    #[test]
    fn fault_reset_is_edge_generated() {
        let mut seq = Cia402::default();
        assert_eq!(seq.controlword(true, true, SW_FAULT), CW_FAULT_RESET);
        // held request: no second edge
        assert_eq!(seq.controlword(true, true, SW_FAULT), CW_DISABLE_VOLTAGE);
        assert_eq!(seq.controlword(true, true, SW_FAULT), CW_DISABLE_VOLTAGE);
        // release re-arms
        assert_eq!(seq.controlword(true, false, SW_FAULT), CW_DISABLE_VOLTAGE);
        assert_eq!(seq.controlword(true, true, SW_FAULT), CW_FAULT_RESET);
    }

    #[test]
    fn fault_reaction_is_left_alone() {
        let mut seq = Cia402::default();
        assert_eq!(seq.controlword(true, true, SW_FAULT_REACTION), CW_DISABLE_VOLTAGE);
    }

    #[test]
    fn drive_status_mapping() {
        assert_eq!(Cia402::drive_status(true, SW_OPERATION_ENABLED), DriveStatus::Enabled);
        assert_eq!(Cia402::drive_status(true, SW_QUICK_STOP_ACTIVE), DriveStatus::QuickStop);
        assert_eq!(Cia402::drive_status(true, SW_FAULT), DriveStatus::Fault);
        assert_eq!(Cia402::drive_status(true, SW_SWITCHED_ON), DriveStatus::Enabling);
        assert_eq!(Cia402::drive_status(false, SW_SWITCHED_ON), DriveStatus::Disabled);
        assert_eq!(Cia402::drive_status(false, SW_SWITCH_ON_DISABLED), DriveStatus::Disabled);
    }
}
