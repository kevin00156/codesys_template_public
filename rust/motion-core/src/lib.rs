//! motion-core: the axis/machine control logic of the Rust motion daemon.
//!
//! Semantics follow the IEC `MC_BasicControl` function block
//! (`codesys_export/.../MC_BasicControl/`): the same step values
//! ([`step::Step`]), error ids and status/command bitmasks
//! ([`flags`]) the HMI already understands, driven by the same level-held
//! request words. Trajectories are generated here ([`profile`], interim
//! trapezoid — rsruckig lands in Phase 2) and streamed to whatever bus is
//! behind the `fieldbus-api` seam; this crate never sees a bus-native type.

pub mod axis;
pub mod flags;
pub mod machine;
pub mod profile;
pub mod step;

pub use axis::{AxisControl, AxisParams, AxisRequest, AxisStatus};
pub use machine::MachineControl;
pub use step::Step;

#[cfg(test)]
mod axis_tests {
    use crate::axis::{AxisControl, AxisParams, AxisRequest};
    use crate::flags::{cmd, error_id, status};
    use crate::step::Step;
    use fieldbus_api::{AxisIn, AxisOut, DriveStatus, Setpoint};

    const DT: f64 = 0.002;

    fn params() -> AxisParams {
        AxisParams {
            max_vel: 100.0,
            acc: 1000.0,
            dec: 1000.0,
            stop_dec: 2000.0,
        }
    }

    /// Perfect drive: instantly enabled when asked, actual == setpoint.
    /// Good enough for state-machine tests; the sim backend models lag.
    fn echo_drive(out: &AxisOut, input: &mut AxisIn) {
        input.drive = if out.enable {
            DriveStatus::Enabled
        } else {
            DriveStatus::Disabled
        };
        match out.setpoint {
            Setpoint::CyclicPosition { pos, vel_ff } => {
                input.act_pos = pos;
                input.act_vel = vel_ff;
            }
            Setpoint::Home => {
                input.act_pos = 0.0;
                input.act_vel = 0.0;
                input.homed = true;
            }
            _ => {
                input.act_vel = 0.0;
            }
        }
    }

    fn word(w: u32) -> AxisRequest {
        AxisRequest {
            word: w,
            ..Default::default()
        }
    }

    /// Run ticks until `pred` or panic after `max` ticks.
    fn run_until(
        axis: &mut AxisControl,
        req: &AxisRequest,
        input: &mut AxisIn,
        max: usize,
        pred: impl Fn(&AxisControl, &AxisIn) -> bool,
    ) {
        for _ in 0..max {
            let out = axis.tick(req, false, input, DT);
            echo_drive(&out, input);
            if pred(axis, input) {
                return;
            }
        }
        panic!("condition not reached, stuck at {:?}", axis.step());
    }

    #[test]
    fn enable_reaches_ready() {
        let mut axis = AxisControl::new(params());
        let mut input = AxisIn::default();

        let out = axis.tick(&word(0), false, &input, DT);
        assert_eq!(axis.step(), Step::Idle);
        assert!(!out.enable);

        let req = word(cmd::ENABLE);
        axis.tick(&req, false, &input, DT);
        assert_eq!(axis.step(), Step::Enabling);
        // one-scan CASE semantics: the ENABLING body runs on the next tick
        let out = axis.tick(&req, false, &input, DT);
        assert!(out.enable, "power request must be asserted in ENABLING");

        input.drive = DriveStatus::Enabled;
        axis.tick(&req, false, &input, DT);
        assert_eq!(axis.step(), Step::Ready);

        // HMI whole-word semantics: a later word without ENABLE must NOT
        // power off (Main.st never re-checks xEnable past IDLE).
        let out = axis.tick(&word(0), false, &input, DT);
        assert_eq!(axis.step(), Step::Ready);
        assert!(out.enable, "enable is sticky past IDLE");
    }

    fn ready_axis() -> (AxisControl, AxisIn) {
        let mut axis = AxisControl::new(params());
        let mut input = AxisIn::default();
        axis.tick(&word(cmd::ENABLE), false, &input, DT);
        input.drive = DriveStatus::Enabled;
        axis.tick(&word(cmd::ENABLE), false, &input, DT);
        assert_eq!(axis.step(), Step::Ready);
        (axis, input)
    }

    #[test]
    fn jog_moves_then_release_stops() {
        let (mut axis, mut input) = ready_axis();
        let jog = AxisRequest {
            word: cmd::JOG_POS,
            jog_vel: 10.0,
            ..Default::default()
        };
        run_until(&mut axis, &jog, &mut input, 10, |a, _| a.step() == Step::JogPos);
        run_until(&mut axis, &jog, &mut input, 1000, |_, i| i.act_vel >= 9.9);
        assert!(input.act_pos > 0.0, "axis must have moved");

        // pointerup → whole word cleared
        let rel = word(0);
        run_until(&mut axis, &rel, &mut input, 1000, |a, _| a.step() == Step::Ready);
        assert!((input.act_vel).abs() < 1e-9);
    }

    #[test]
    fn move_abs_completes_and_rearms_only_on_fresh_word() {
        let (mut axis, mut input) = ready_axis();
        let go = AxisRequest {
            word: cmd::MOVE_ABS,
            move_abs_pos: 5.0,
            move_abs_vel: 50.0,
            fresh: true,
            ..Default::default()
        };
        let out = axis.tick(&go, false, &input, DT);
        echo_drive(&out, &mut input);
        assert_eq!(axis.step(), Step::MoveAbs);

        // Keep the word latched (fresh only on the first tick) until done.
        let held = AxisRequest { fresh: false, ..go };
        run_until(&mut axis, &held, &mut input, 5000, |a, _| a.step() == Step::Ready);
        assert!(
            (input.act_pos - 5.0).abs() < 1e-9,
            "landed at {}",
            input.act_pos
        );

        // Latched (stale) word must NOT restart the move.
        for _ in 0..50 {
            let out = axis.tick(&held, false, &input, DT);
            echo_drive(&out, &mut input);
            assert_eq!(axis.step(), Step::Ready, "stale MOVE_ABS bit re-dispatched");
        }

        // A fresh word (second Go click, same target allowed) restarts.
        let go2 = AxisRequest {
            move_abs_pos: -2.0,
            fresh: true,
            ..go
        };
        let out = axis.tick(&go2, false, &input, DT);
        echo_drive(&out, &mut input);
        assert_eq!(axis.step(), Step::MoveAbs);
        let held2 = AxisRequest { fresh: false, ..go2 };
        run_until(&mut axis, &held2, &mut input, 5000, |a, _| a.step() == Step::Ready);
        assert!((input.act_pos + 2.0).abs() < 1e-9);
    }

    #[test]
    fn move_abs_zero_velocity_raises_error() {
        let (mut axis, mut input) = ready_axis();
        let bad = AxisRequest {
            word: cmd::MOVE_ABS,
            move_abs_pos: 5.0,
            move_abs_vel: 0.0,
            fresh: true,
            ..Default::default()
        };
        let out = axis.tick(&bad, false, &input, DT);
        echo_drive(&out, &mut input);
        assert_eq!(axis.step(), Step::TryReset);
        assert_eq!(axis.status(&input).error_id, error_id::MOVE_ABS_FAIL);
    }

    #[test]
    fn ems_interrupts_jog_and_recovers() {
        let (mut axis, mut input) = ready_axis();
        let jog = AxisRequest {
            word: cmd::JOG_POS,
            jog_vel: 10.0,
            ..Default::default()
        };
        run_until(&mut axis, &jog, &mut input, 1000, |_, i| i.act_vel >= 9.9);

        // EMS latched (machine word bit1) → forced into STOPPING, decel.
        for _ in 0..1000 {
            let out = axis.tick(&jog, true, &input, DT);
            echo_drive(&out, &mut input);
            if input.act_vel == 0.0 {
                break;
            }
        }
        assert_eq!(axis.step(), Step::Stopping, "EMS freezes the machine in STOPPING");
        assert_eq!(input.act_vel, 0.0, "EMS must decelerate to standstill");

        // HMI Reset click replaces the machine word → EMS released; axis
        // word also replaced (jog gone) → back to READY.
        let rel = word(0);
        run_until(&mut axis, &rel, &mut input, 1000, |a, _| a.step() == Step::Ready);
    }

    #[test]
    fn limit_trip_errors_then_hmi_reset_recovers() {
        let (mut axis, mut input) = ready_axis();
        let jog = AxisRequest {
            word: cmd::JOG_POS,
            jog_vel: 10.0,
            ..Default::default()
        };
        run_until(&mut axis, &jog, &mut input, 1000, |_, i| i.act_vel >= 9.9);

        input.pos_limit = true;
        let out = axis.tick(&jog, false, &input, DT);
        echo_drive(&out, &mut input);
        assert_eq!(axis.step(), Step::TryReset);
        let st = axis.status(&input);
        assert_eq!(st.error_id, error_id::POSITIVE_LIMIT_TRIP);
        assert!(st.flags & status::ERROR != 0);

        // Without HMI reset it stays put (decelerating to standstill).
        for _ in 0..200 {
            let out = axis.tick(&word(0), false, &input, DT);
            echo_drive(&out, &mut input);
        }
        assert_eq!(axis.step(), Step::TryReset);

        // Reset click (limit released — e.g. axis backed off physically).
        input.pos_limit = false;
        let reset = word(cmd::RESET);
        run_until(&mut axis, &reset, &mut input, 1000, |a, _| a.step() == Step::Ready);
        assert_eq!(axis.status(&input).error_id, error_id::NONE);

        // READY must refuse to re-enter jog toward a held limit.
        input.pos_limit = true;
        let out = axis.tick(&jog, false, &input, DT);
        echo_drive(&out, &mut input);
        assert_eq!(axis.step(), Step::Ready);
    }

    #[test]
    fn drive_fault_latches_driver_error() {
        let (mut axis, mut input) = ready_axis();
        input.drive = DriveStatus::Fault;
        input.fault_code = 0x7500;
        axis.tick(&word(0), false, &input, DT);
        assert_eq!(axis.step(), Step::TryReset);
        assert_eq!(axis.status(&input).error_id, error_id::DRIVER_ERROR);

        // Reset while the fault persists: no recovery.
        for _ in 0..50 {
            axis.tick(&word(cmd::RESET), false, &input, DT);
        }
        assert_eq!(axis.step(), Step::TryReset);

        // Drive healthy again + reset → READY.
        input.drive = DriveStatus::Enabled;
        input.fault_code = 0;
        input.act_vel = 0.0;
        let mut ok = false;
        for _ in 0..200 {
            axis.tick(&word(cmd::RESET), false, &input, DT);
            if axis.step() == Step::Ready {
                ok = true;
                break;
            }
        }
        assert!(ok, "reset with healthy drive must reach READY");
    }

    #[test]
    fn home_completes_and_rearms() {
        let (mut axis, mut input) = ready_axis();
        input.act_pos = 42.0;
        let home = AxisRequest {
            word: cmd::HOME,
            fresh: true,
            ..Default::default()
        };
        let out = axis.tick(&home, false, &input, DT);
        echo_drive(&out, &mut input);
        assert_eq!(axis.step(), Step::HomingWriteParam);

        let held = AxisRequest { fresh: false, ..home };
        run_until(&mut axis, &held, &mut input, 1000, |a, _| a.step() == Step::Ready);
        assert!(input.homed);
        assert_eq!(input.act_pos, 0.0);

        // Latched HOME bit must not re-home.
        for _ in 0..50 {
            let out = axis.tick(&held, false, &input, DT);
            echo_drive(&out, &mut input);
            assert_eq!(axis.step(), Step::Ready);
        }
    }
}
