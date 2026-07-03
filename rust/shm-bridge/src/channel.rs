//! Daemon-side conveniences over the raw seqlock: a publisher that owns the
//! `plc_data` segment and a command reader with `PRG_ShmPublisher.st`'s
//! latch-on-cycle-change semantics.

use crate::layout::{PlcCommand, PlcData, Segment};
use crate::mapping::Mapping;
use crate::seqlock::{self, ReadError};

/// Owns the `plc_data` segment. Mutate [`DataPublisher::data`] freely, then
/// [`publish`](DataPublisher::publish) once per cycle; magic/version are
/// maintained here, `header.cycle` is bumped per publish (the IEC program
/// did the same), `header.seq` is owned by the seqlock writer.
pub struct DataPublisher {
    map: Mapping,
    pub data: PlcData,
}

impl DataPublisher {
    pub fn new(map: Mapping) -> DataPublisher {
        let mut data = PlcData::default();
        data.header.magic = PlcData::MAGIC;
        data.header.version = PlcData::VERSION;
        DataPublisher { map, data }
    }

    pub fn publish(&mut self) {
        self.data.header.cycle = self.data.header.cycle.wrapping_add(1);
        seqlock::publish(&self.map, &self.data);
    }
}

/// Reads `plc_cmd` with the exact consumption semantics of
/// `PRG_ShmPublisher.st`: seqlock-snapshot each cycle, and only when
/// `header.cycle` changed does the snapshot become the *current* command
/// (whole-struct latch). Between commands the last latched word stays in
/// force — that is what makes the HMI's level-held flags (jog pressed,
/// EMS latched) work.
pub struct CmdReader {
    map: Mapping,
    last_cycle: u64,
    current: PlcCommand,
    valid: bool,
}

impl CmdReader {
    pub fn new(map: Mapping) -> CmdReader {
        CmdReader {
            map,
            last_cycle: 0,
            current: PlcCommand::default(),
            valid: false,
        }
    }

    /// Poll the segment. `Ok(true)` = a new command was latched this call;
    /// `Ok(false)` = nothing new. Errors (no writer yet, torn, version skew)
    /// leave the previous latch in force, mirroring the IEC reader that
    /// keeps `bCmdValid`'s last good snapshot.
    pub fn poll(&mut self) -> Result<bool, ReadError> {
        let mut snap = PlcCommand::default();
        seqlock::snapshot(&self.map, &mut snap)?;
        if !self.valid || snap.header.cycle != self.last_cycle {
            self.last_cycle = snap.header.cycle;
            self.current = snap;
            self.valid = true;
            return Ok(true);
        }
        Ok(false)
    }

    /// Last latched command (zeroed until the first successful latch —
    /// all-flags-clear, which is the safe default).
    pub fn current(&self) -> &PlcCommand {
        &self.current
    }
}
