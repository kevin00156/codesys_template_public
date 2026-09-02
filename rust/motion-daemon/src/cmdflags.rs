//! Bit vocabulary the daemon adds on top of `motion_core::flags` (which
//! mirrors the IEC/Go layout comments): the per-axis touch mask in
//! `PlcCommand.header.flags`, and the daemon-defined bits of
//! `PlcData.system.status_flags` / `alarm_flags`. The Go bridge is the other
//! party to every one of these — keep `backend/internal/shm` in sync.

// ─── PlcCommand.header.flags ─────────────────────────────────────────────────

/// Bit 15: the writer filled the per-axis touch mask (bits 0..3). A message
/// without it comes from a legacy writer and must keep today's behaviour —
/// every axis counts as touched by every message.
pub const TOUCH_MASK_PRESENT: u16 = 0x8000;

/// Bit `axis` (0..3): axis `axis`'s `AxisCmd` was written by this message.
/// Only meaningful together with [`TOUCH_MASK_PRESENT`].
pub const fn touch_bit(axis: usize) -> u16 {
    1 << axis
}

/// Per-axis "this message is a new request for axis `axis`" — the edge
/// `motion_core`'s one-shot re-arm (MOVE_ABS / HOME) keys off. `fresh` is
/// the reader's whole-message edge (`header.cycle` changed).
///
/// Without the mask, an HMI click on axis 1 would re-dispatch axis 0's
/// still-latched MOVE_ABS word (the bridge publishes the whole struct);
/// with it, only the axes the message actually touched see a fresh edge.
pub fn axis_fresh(fresh: bool, header_flags: u16, axis: usize) -> bool {
    fresh
        && (header_flags & TOUCH_MASK_PRESENT == 0 || header_flags & touch_bit(axis) != 0)
}

// ─── PlcData.system ──────────────────────────────────────────────────────────

/// `status_flags` bit 0: the bus is in OP.
pub const STATUS_BUS_OP: u32 = 1 << 0;

/// `alarm_flags` bit 0: the last cyclic exchange failed. `plc_data` keeps
/// publishing through the fault (cycle counter advancing, last known axis
/// status) so the HMI can tell "bus down" from "daemon dead".
pub const ALARM_BUS_FAULT: u32 = 1 << 0;

/// `alarm_flags` bit 1: the command dead-man tripped — a jog word is latched
/// but no command message arrived within `cmd_timeout` (the bridge died with
/// jog pressed). The daemon strips the jog bits until the next fresh message
/// re-arms it; level-held bits like EMS stay in force.
pub const ALARM_CMD_TIMEOUT: u32 = 1 << 1;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_writer_touches_every_axis() {
        for axis in 0..4 {
            assert!(axis_fresh(true, 0, axis));
            assert!(!axis_fresh(false, 0, axis));
        }
    }

    #[test]
    fn touch_mask_selects_axes() {
        let flags = TOUCH_MASK_PRESENT | touch_bit(1) | touch_bit(3);
        assert!(!axis_fresh(true, flags, 0));
        assert!(axis_fresh(true, flags, 1));
        assert!(!axis_fresh(true, flags, 2));
        assert!(axis_fresh(true, flags, 3));
        // Marker without any axis bit: a message that touched nothing
        // (e.g. machine-level only) is fresh for no axis.
        assert!(!axis_fresh(true, TOUCH_MASK_PRESENT, 0));
        // Not fresh at all beats any mask.
        assert!(!axis_fresh(false, flags, 1));
    }
}
