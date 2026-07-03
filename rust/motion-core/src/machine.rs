//! Machine-level control: fans the latched machine command out to the axes
//! and derives the machine-level published fields. Deliberately IPC-agnostic
//! — the daemon translates shm structs to/from these plain types.

use fieldbus_api::{AxisIn, AxisOut};

use crate::axis::{AxisControl, AxisParams, AxisRequest, AxisStatus};
use crate::flags::machine_cmd;

pub struct MachineControl {
    axes: Vec<AxisControl>,
}

impl MachineControl {
    pub fn new(axis_count: usize, params: AxisParams) -> MachineControl {
        MachineControl {
            axes: (0..axis_count).map(|_| AxisControl::new(params)).collect(),
        }
    }

    pub fn axis_count(&self) -> usize {
        self.axes.len()
    }

    /// One machine cycle. `machine_word` is the level-held
    /// `MachineCmd.ControlFlags`; EMS (bit1) latches until the HMI replaces
    /// the word (its Reset button writes bit0, releasing EMS).
    pub fn tick(
        &mut self,
        machine_word: u32,
        reqs: &[AxisRequest],
        ins: &[AxisIn],
        dt: f64,
        outs: &mut [AxisOut],
    ) {
        let ems = machine_word & machine_cmd::EMS != 0;
        for (i, axis) in self.axes.iter_mut().enumerate() {
            outs[i] = axis.tick(&reqs[i], ems, &ins[i], dt);
        }
    }

    pub fn status(&self, i: usize, input: &AxisIn) -> AxisStatus {
        self.axes[i].status(input)
    }

    /// `MachineState.RunState` — project-defined enum; the template keeps it
    /// binary (SystemRun latched or not), machine branches refine it.
    pub fn run_state(machine_word: u32) -> u32 {
        u32::from(machine_word & machine_cmd::SYSTEM_RUN != 0)
    }
}
