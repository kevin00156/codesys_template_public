//! Per-axis state machine with `MC_BasicControl.Main.st` semantics.
//!
//! The shm command word is *level-held*: the HMI writes a whole
//! `AxisCmd.ControlFlags` word per interaction and the daemon latches it
//! until the next one arrives (`CmdReader` mirrors `PRG_ShmPublisher.st`).
//! That reproduces the FB's request-flag pattern — a held jog button is a
//! continuously-set flag, releasing it clears the word — with two documented
//! deviations (see `rust/README.md` §Deviations):
//!
//! 1. `MOVE_ABS` re-arms like the FB's `MOVE_REL` (`_xMoveRelNeedsReArm`):
//!    a completed move does not restart from the still-latched flag; only a
//!    *fresh* command word (new `header.cycle`) with the bit set starts the
//!    next move. Without this, the latched bit would bounce the step through
//!    MOVE_ABS→STOPPING→READY forever.
//! 2. Error recovery always requires the HMI `RESET` bit while in
//!    `TRY_RESET` (the FB auto-retries some paths); explicit reset is
//!    deterministic and safer over a 50–100 ms HMI link.

use fieldbus_api::{AxisIn, AxisOut, DriveStatus, Setpoint};

use crate::flags::{cmd, error_id, status};
use crate::profile::{ramp_vel, trapezoid_tick};
use crate::step::Step;

/// Actual velocity below this (user units/s) counts as standing still.
const STANDSTILL_EPS: f64 = 1e-3;

/// Kinematic limits an axis enforces (`stParameter` equivalent; the shm
/// command carries only target/velocity, accelerations are configuration).
#[derive(Clone, Copy, Debug)]
pub struct AxisParams {
    /// Velocity clamp for every command (user units/s).
    pub max_vel: f64,
    /// Acceleration for jog / move ramps (user units/s²).
    pub acc: f64,
    /// Deceleration for move profiles (user units/s²).
    pub dec: f64,
    /// Deceleration for MC_Stop / EMS ramps (user units/s²).
    pub stop_dec: f64,
}

impl Default for AxisParams {
    fn default() -> Self {
        AxisParams {
            max_vel: 1000.0,
            acc: 500.0,
            dec: 500.0,
            stop_dec: 2000.0,
        }
    }
}

/// One axis' slice of the latched command, decoded for a tick.
#[derive(Clone, Copy, Debug, Default)]
pub struct AxisRequest {
    /// Raw `AxisCmd.ControlFlags` word (level-held).
    pub word: u32,
    pub jog_vel: f64,
    pub move_abs_pos: f64,
    pub move_abs_vel: f64,
    /// True on the tick a new command message was latched
    /// (`header.cycle` changed) — the "the caller called a method this
    /// scan" edge that one-shot requests key off.
    pub fresh: bool,
}

impl AxisRequest {
    fn has(&self, bit: u32) -> bool {
        self.word & bit != 0
    }
}

/// Published status of one axis — becomes `shm::AxisState` verbatim.
#[derive(Clone, Copy, Debug, Default)]
pub struct AxisStatus {
    pub step: i32,
    pub flags: u32,
    pub error_id: i32,
    pub set_pos: f64,
    pub set_vel: f64,
}

pub struct AxisControl {
    params: AxisParams,
    step: Step,
    busy: bool,
    error: bool,
    error_id: i32,
    /// Generated setpoints (the FB's SetPosition/SetVelocity outputs).
    set_pos: f64,
    set_vel: f64,
    move_target: f64,
    move_vmax: f64,
    /// One-shot re-arm latches (deviation 1 above): set when the command
    /// completes, cleared by the next *fresh* command word. A latched bit
    /// alone never restarts a finished MOVE_ABS / HOME.
    move_abs_rearm: bool,
    home_rearm: bool,
    out_enable: bool,
    out_fault_reset: bool,
}

impl AxisControl {
    pub fn new(params: AxisParams) -> AxisControl {
        AxisControl {
            params,
            step: Step::Idle,
            busy: false,
            error: false,
            error_id: error_id::NONE,
            set_pos: 0.0,
            set_vel: 0.0,
            move_target: 0.0,
            move_vmax: 0.0,
            move_abs_rearm: false,
            home_rearm: false,
            out_enable: false,
            out_fault_reset: false,
        }
    }

    pub fn step(&self) -> Step {
        self.step
    }

    /// Raise an error the way the FB's `Raise()` does: latch id + move the
    /// step. Recovery happens in TRY_RESET.
    fn raise(&mut self, id: i32) {
        self.error = true;
        self.error_id = id;
        self.step = Step::TryReset;
    }

    fn standstill(&self, input: &AxisIn) -> bool {
        self.set_vel == 0.0 && input.act_vel.abs() < STANDSTILL_EPS
    }

    /// Decelerate the generated setpoint toward zero velocity (MC_Stop /
    /// MC_Stop_EMS equivalent). Returns true once the ramp is done.
    fn ramp_to_stop(&mut self, dec: f64, dt: f64) -> bool {
        self.set_vel = ramp_vel(self.set_vel, 0.0, dec, dt);
        self.set_pos += self.set_vel * dt;
        self.set_vel == 0.0
    }

    /// One control cycle. Mirrors the section order of
    /// `MC_BasicControl.Main.st`; `ems` is the machine-level EMS flag
    /// (level-held from `MachineCmd.ControlFlags` bit1).
    pub fn tick(&mut self, req: &AxisRequest, ems: bool, input: &AxisIn, dt: f64) -> AxisOut {
        self.out_fault_reset = false;

        // A new command message is a new request: re-arm the one-shots.
        // (The bits themselves stay level-held; see the module docs.)
        if req.fresh {
            self.move_abs_rearm = false;
            self.home_rearm = false;
        }

        // ── 4. drive error detection (Main.st `_DetectErrors`) ──────────
        // A drive fault anywhere past IDLE latches DRIVER_ERROR; TRY_RESET
        // owns recovery.
        if input.drive == DriveStatus::Fault
            && !self.error
            && !matches!(self.step, Step::Idle | Step::Enabling | Step::NotReady)
        {
            self.raise(error_id::DRIVER_ERROR);
        }

        // ── 5. EMS: decelerate, force motion steps into STOPPING, freeze
        // the state machine while held (Main.st EMS block + MC_Stop_EMS) ──
        if ems {
            self.ramp_to_stop(self.params.stop_dec, dt);
            if self.step.is_motion() {
                self.step = Step::Stopping;
            }
            self.busy = self.step != Step::Idle && self.step != Step::Ready;
            return self.outputs(input);
        }

        // ── 6. the state machine ─────────────────────────────────────────
        match self.step {
            Step::Idle => {
                self.out_enable = false;
                self.busy = false;
                self.set_pos = input.act_pos; // track the axis while unpowered
                self.set_vel = 0.0;
                if req.has(cmd::ENABLE) {
                    self.step = Step::Enabling;
                }
            }

            Step::Enabling => {
                self.out_enable = true;
                self.busy = true;
                self.set_pos = input.act_pos;
                self.set_vel = 0.0;
                if input.drive == DriveStatus::Enabled {
                    self.step = Step::Ready;
                } else if input.drive == DriveStatus::Fault {
                    self.step = Step::NotReady;
                }
            }

            Step::NotReady => {
                self.out_enable = true;
                self.busy = true;
                self.out_fault_reset = true;
                self.set_pos = input.act_pos;
                self.set_vel = 0.0;
                if input.drive == DriveStatus::Enabled && input.fault_code == 0 {
                    self.step = Step::Ready;
                }
            }

            Step::Ready => {
                self.busy = false;
                // Dispatch priority mirrors Main.st READY (MoveAbs > Jog >
                // Home; MoveRel/MoveVel sit between but are not expressible
                // in shm AxisCmd v3).
                if req.has(cmd::MOVE_ABS) && !self.move_abs_rearm {
                    if req.move_abs_vel <= 0.0 {
                        self.raise(error_id::MOVE_ABS_FAIL);
                    } else {
                        self.move_target = req.move_abs_pos;
                        self.move_vmax = req.move_abs_vel.min(self.params.max_vel);
                        self.step = Step::MoveAbs;
                    }
                } else if req.has(cmd::JOG_POS) && !input.pos_limit {
                    self.step = Step::JogPos;
                } else if req.has(cmd::JOG_NEG) && !input.neg_limit {
                    self.step = Step::JogNeg;
                } else if req.has(cmd::HOME) && !self.home_rearm {
                    self.step = Step::HomingWriteParam;
                }
            }

            Step::JogPos | Step::JogNeg => {
                self.busy = true;
                let dir = if self.step == Step::JogPos { 1.0 } else { -1.0 };
                let (own_bit, limit, trip) = if dir > 0.0 {
                    (cmd::JOG_POS, input.pos_limit, error_id::POSITIVE_LIMIT_TRIP)
                } else {
                    (cmd::JOG_NEG, input.neg_limit, error_id::NEGATIVE_LIMIT_TRIP)
                };
                if limit {
                    self.raise(trip);
                } else if !req.has(own_bit) {
                    self.step = Step::Stopping;
                } else {
                    let v = dir * req.jog_vel.abs().min(self.params.max_vel);
                    self.set_vel = ramp_vel(self.set_vel, v, self.params.acc, dt);
                    self.set_pos += self.set_vel * dt;
                }
            }

            Step::MoveAbs => {
                self.busy = true;
                if !req.has(cmd::MOVE_ABS) {
                    // caller stopped calling MoveAbsolute() → cancel
                    self.step = Step::Stopping;
                } else {
                    let (p, v, done) = trapezoid_tick(
                        self.set_pos,
                        self.set_vel,
                        self.move_target,
                        self.move_vmax,
                        self.params.acc,
                        self.params.dec,
                        dt,
                    );
                    self.set_pos = p;
                    self.set_vel = v;
                    if done {
                        self.move_abs_rearm = true;
                        self.step = Step::Stopping;
                    }
                }
            }

            Step::HomingWriteParam => {
                self.busy = true;
                // Homing parameters travel over the acyclic channel before
                // Setpoint::Home is asserted; nothing to write for the sim
                // backend, and the EtherCAT flow lands here in Phase 3.
                if !req.has(cmd::HOME) {
                    self.step = Step::Stopping;
                } else {
                    self.step = Step::HomingExec;
                }
            }

            Step::HomingExec => {
                self.busy = true;
                self.set_vel = 0.0; // trajectory is drive/backend-owned during homing
                if input.homed {
                    // position reference jumped: re-seat the setpoint
                    self.set_pos = input.act_pos;
                    self.home_rearm = true;
                    self.step = Step::Stopping;
                } else if !req.has(cmd::HOME) {
                    self.step = Step::Stopping;
                }
            }

            Step::Stopping => {
                self.busy = true;
                if self.ramp_to_stop(self.params.stop_dec, dt) {
                    self.step = Step::WaitStop;
                }
            }

            Step::WaitStop => {
                self.busy = true;
                if self.standstill(input) && !req.has(cmd::STOP) {
                    self.step = Step::Ready;
                }
            }

            Step::TryReset => {
                self.busy = true;
                self.ramp_to_stop(self.params.stop_dec, dt);
                if req.has(cmd::RESET) {
                    self.out_fault_reset = true;
                }
                let healthy = input.drive != DriveStatus::Fault && input.fault_code == 0;
                if req.has(cmd::RESET) && healthy && self.standstill(input) {
                    self.error = false;
                    self.error_id = error_id::NONE;
                    self.set_pos = input.act_pos;
                    self.step = Step::Ready;
                }
            }

            // Unreachable via shm AxisCmd v3; parked on the enum for layout
            // fidelity with the IEC step values.
            Step::MoveRel | Step::MoveVel | Step::SetPosition => {
                self.step = Step::Stopping;
            }
        }

        self.outputs(input)
    }

    fn outputs(&self, input: &AxisIn) -> AxisOut {
        let setpoint = if !self.out_enable {
            Setpoint::Hold
        } else if self.step == Step::HomingExec {
            Setpoint::Home
        } else if input.drive == DriveStatus::Enabled {
            // CSP stream — Phase 1 supports SetpointKind::CyclicTrajectory
            // backends; the TargetForwarding strategy branch arrives with
            // fieldbus-modbus (see README ADR-3).
            Setpoint::CyclicPosition {
                pos: self.set_pos,
                vel_ff: self.set_vel,
            }
        } else {
            Setpoint::Hold
        };
        AxisOut {
            enable: self.out_enable,
            fault_reset: self.out_fault_reset,
            setpoint,
        }
    }

    /// Status snapshot for publishing (act pos/vel come straight from the
    /// bus in the daemon's publish step).
    pub fn status(&self, input: &AxisIn) -> AxisStatus {
        let mut flags = 0u32;
        if input.drive == DriveStatus::Enabled {
            flags |= status::ENABLED;
        }
        if self.busy {
            flags |= status::BUSY;
        }
        if self.error {
            flags |= status::ERROR;
        }
        if self.standstill(input) {
            flags |= status::STANDSTILL;
        }
        if input.pos_limit {
            flags |= status::POS_LIMIT;
        }
        if input.neg_limit {
            flags |= status::NEG_LIMIT;
        }
        AxisStatus {
            step: self.step.as_i32(),
            flags,
            error_id: self.error_id,
            set_pos: self.set_pos,
            set_vel: self.set_vel,
        }
    }
}
