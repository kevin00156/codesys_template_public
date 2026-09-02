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
    use crate::machine::MachineControl;
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
        ready_axis_at(0.0)
    }

    /// READY with the axis physically standing at `pos` (ENABLING seats the
    /// setpoint there, as the real enable path does).
    fn ready_axis_at(pos: f64) -> (AxisControl, AxisIn) {
        let mut axis = AxisControl::new(params());
        let mut input = AxisIn {
            act_pos: pos,
            ..Default::default()
        };
        axis.tick(&word(cmd::ENABLE), false, &input, DT);
        input.drive = DriveStatus::Enabled;
        axis.tick(&word(cmd::ENABLE), false, &input, DT);
        assert_eq!(axis.step(), Step::Ready);
        (axis, input)
    }

    /// A CSP setpoint away from where the axis actually is would be a
    /// physical jump; `Hold`/`Home` never are.
    fn assert_no_jump(out: &AxisOut, input: &AxisIn, ctx: &str) {
        if let Setpoint::CyclicPosition { pos, .. } = out.setpoint {
            assert!(
                (pos - input.act_pos).abs() < 1e-9,
                "{ctx}: CSP setpoint {pos} jumps away from act_pos {}",
                input.act_pos
            );
        }
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

    /// Deviation 3: the drive leaving Operation Enabled mid-motion (not
    /// only a Fault) must latch DRIVER_ERROR instead of letting the CSP
    /// integrator run away from a motor that is no longer following.
    #[test]
    fn disabled_drive_mid_jog_raises_driver_error() {
        for dropped in [
            DriveStatus::Disabled,
            DriveStatus::QuickStop,
            DriveStatus::Offline,
        ] {
            let ctx = format!("{dropped:?}");
            let (mut axis, mut input) = ready_axis();
            let jog = AxisRequest {
                word: cmd::JOG_POS,
                jog_vel: 10.0,
                ..Default::default()
            };
            run_until(&mut axis, &jog, &mut input, 1000, |_, i| i.act_vel >= 9.9);

            // The power stage drops out: the motor stops where it is.
            input.drive = dropped;
            input.act_vel = 0.0;
            let stopped_at = input.act_pos;
            let out = axis.tick(&jog, false, &input, DT);
            assert_eq!(axis.step(), Step::TryReset, "{ctx}");
            let st = axis.status(&input);
            assert_eq!(st.error_id, error_id::DRIVER_ERROR, "{ctx}");
            assert!(st.flags & status::ERROR != 0, "{ctx}");
            assert_no_jump(&out, &input, &ctx);

            // Still out, jog released: stays in TRY_RESET, setpoint glued
            // to the actual position the whole time (no integration).
            for _ in 0..50 {
                let out = axis.tick(&word(0), false, &input, DT);
                assert_no_jump(&out, &input, &ctx);
            }
            assert_eq!(axis.step(), Step::TryReset, "{ctx}");
            assert_eq!(axis.status(&input).set_pos, stopped_at, "{ctx}");

            // Drive back in Operation Enabled + HMI Reset → READY, and the
            // first CSP setpoints the drive sees are where it already is.
            input.drive = DriveStatus::Enabled;
            let reset = word(cmd::RESET);
            let mut reached = false;
            for _ in 0..200 {
                let out = axis.tick(&reset, false, &input, DT);
                assert_no_jump(&out, &input, &ctx);
                echo_drive(&out, &mut input);
                if axis.step() == Step::Ready {
                    reached = true;
                    break;
                }
            }
            assert!(reached, "{ctx}: reset with re-enabled drive must reach READY");
            let st = axis.status(&input);
            assert_eq!(st.error_id, error_id::NONE, "{ctx}");
            assert_eq!(st.set_pos, input.act_pos, "{ctx}");
            assert_eq!(input.act_pos, stopped_at, "{ctx}: axis must not have moved");
        }
    }

    /// Deviation 3, reset path: a drive that is fault-free but not yet back
    /// in Operation Enabled must not release TRY_RESET.
    #[test]
    fn try_reset_waits_for_drive_enabled() {
        let (mut axis, mut input) = ready_axis();
        input.drive = DriveStatus::Fault;
        input.fault_code = 0x7500;
        axis.tick(&word(0), false, &input, DT);
        assert_eq!(axis.step(), Step::TryReset);

        // Fault cleared, adapter re-running the enable handshake.
        input.drive = DriveStatus::Enabling;
        input.fault_code = 0;
        for _ in 0..50 {
            axis.tick(&word(cmd::RESET), false, &input, DT);
        }
        assert_eq!(axis.step(), Step::TryReset, "READY against an Enabling drive would re-raise");

        input.drive = DriveStatus::Enabled;
        axis.tick(&word(cmd::RESET), false, &input, DT);
        assert_eq!(axis.step(), Step::Ready);
    }

    /// Deviation 4: cancelling a drive-owned homing motion (HOME released,
    /// or EMS) must start the stop ramp from the actual position, not from
    /// the setpoint left behind when homing started.
    #[test]
    fn homing_cancel_stops_from_actual_position() {
        for via_ems in [false, true] {
            let ctx = if via_ems { "EMS" } else { "HOME released" };
            let (mut axis, mut input) = ready_axis_at(42.0);
            let home = AxisRequest {
                word: cmd::HOME,
                fresh: true,
                ..Default::default()
            };
            let out = axis.tick(&home, false, &input, DT);
            assert_eq!(axis.step(), Step::HomingWriteParam, "{ctx}");
            assert_no_jump(&out, &input, ctx);

            let held = AxisRequest { fresh: false, ..home };
            let out = axis.tick(&held, false, &input, DT);
            assert_eq!(axis.step(), Step::HomingExec, "{ctx}");
            assert_eq!(out.setpoint, Setpoint::Home, "{ctx}");

            // Drive-side homing: creeps toward the switch at 0 (0.5 per
            // tick) under Setpoint::Home; never reports `homed` because we
            // cancel first.
            let creep = |input: &mut AxisIn| {
                input.act_pos -= 0.5;
                input.act_vel = -0.5 / DT;
            };
            for _ in 0..40 {
                creep(&mut input);
                let out = axis.tick(&held, false, &input, DT);
                assert_eq!(axis.step(), Step::HomingExec, "{ctx}");
                assert_eq!(out.setpoint, Setpoint::Home, "{ctx}");
            }
            assert!((input.act_pos - 22.0).abs() < 1e-9, "{ctx}");

            // Cancel: the stop ramp must pick the axis up where it is.
            creep(&mut input);
            let (req, ems) = if via_ems { (held, true) } else { (word(0), false) };
            let out = axis.tick(&req, ems, &input, DT);
            assert_eq!(axis.step(), Step::Stopping, "{ctx}");
            match out.setpoint {
                Setpoint::CyclicPosition { pos, .. } => assert!(
                    (pos - input.act_pos).abs() < 1e-9,
                    "{ctx}: stop ramp starts at {pos}, axis is at {}",
                    input.act_pos
                ),
                other => panic!("{ctx}: expected CSP stream after cancel, got {other:?}"),
            }
            let cancelled_at = input.act_pos;

            // Drive follows the CSP stream from here (EMS released by the
            // HMI Reset replacing the machine word): READY, no movement.
            echo_drive(&out, &mut input);
            run_until(&mut axis, &word(0), &mut input, 1000, |a, _| a.step() == Step::Ready);
            assert!((input.act_pos - cancelled_at).abs() < 1e-9, "{ctx}: axis moved after cancel");
            assert!(!input.homed, "{ctx}");
        }
    }

    /// Deviation 5: a fresh MOVE_ABS word mid-move re-targets the running
    /// move (continuous profile, reversal included); an invalid velocity is
    /// rejected the way READY rejects it.
    #[test]
    fn move_abs_retargets_on_fresh_word() {
        let (mut axis, mut input) = ready_axis();
        let go = AxisRequest {
            word: cmd::MOVE_ABS,
            move_abs_pos: 50.0,
            move_abs_vel: 20.0,
            fresh: true,
            ..Default::default()
        };
        let out = axis.tick(&go, false, &input, DT);
        echo_drive(&out, &mut input);
        assert_eq!(axis.step(), Step::MoveAbs);
        let held = AxisRequest { fresh: false, ..go };
        for _ in 0..200 {
            let out = axis.tick(&held, false, &input, DT);
            echo_drive(&out, &mut input);
        }
        assert_eq!(axis.step(), Step::MoveAbs);
        assert!(input.act_vel > 19.0, "should be cruising, act_vel={}", input.act_vel);
        assert!(input.act_pos > 5.0 && input.act_pos < 50.0);

        // Second Go click while moving: new target behind us.
        let retarget = AxisRequest {
            move_abs_pos: -10.0,
            fresh: true,
            ..go
        };
        let out = axis.tick(&retarget, false, &input, DT);
        echo_drive(&out, &mut input);
        assert_eq!(axis.step(), Step::MoveAbs);

        // Held afterwards (fresh only on that one tick). The echo drive
        // follows the CSP stream, so any discontinuity would show up as a
        // per-tick step larger than vmax·dt.
        let held2 = AxisRequest { fresh: false, ..retarget };
        let mut prev = input.act_pos;
        let mut reversed = false;
        let mut reached = false;
        for _ in 0..20_000 {
            let out = axis.tick(&held2, false, &input, DT);
            echo_drive(&out, &mut input);
            assert!(
                (input.act_pos - prev).abs() <= 20.0 * DT + 1e-9,
                "position discontinuity {prev} → {}",
                input.act_pos
            );
            prev = input.act_pos;
            reversed |= input.act_vel < 0.0;
            if axis.step() == Step::Ready {
                reached = true;
                break;
            }
        }
        assert!(reached, "stuck at {:?}", axis.step());
        assert!(reversed, "the move must have turned around");
        assert!((input.act_pos + 10.0).abs() < 1e-9, "landed at {}", input.act_pos);

        // Re-trigger with velocity 0 mid-move: MOVE_ABS_FAIL, like READY.
        let (mut axis, mut input) = ready_axis();
        let out = axis.tick(&go, false, &input, DT);
        echo_drive(&out, &mut input);
        for _ in 0..50 {
            let out = axis.tick(&held, false, &input, DT);
            echo_drive(&out, &mut input);
        }
        assert_eq!(axis.step(), Step::MoveAbs);
        let bad = AxisRequest {
            move_abs_vel: 0.0,
            fresh: true,
            ..go
        };
        let out = axis.tick(&bad, false, &input, DT);
        echo_drive(&out, &mut input);
        assert_eq!(axis.step(), Step::TryReset);
        assert_eq!(axis.status(&input).error_id, error_id::MOVE_ABS_FAIL);
    }

    /// `fresh` is a per-axis edge: another axis' new command message must
    /// not re-arm this axis' completed one-shot. The daemon computes
    /// `fresh` per axis from the command header's touch mask and relies on
    /// MachineControl passing it through untouched.
    #[test]
    fn machine_fresh_is_per_axis() {
        let mut m = MachineControl::new(2, params());
        let mut ins = [AxisIn::default(); 2];
        let mut outs = [AxisOut::default(); 2];
        let step_of = |m: &MachineControl, i: usize, ins: &[AxisIn; 2]| m.status(i, &ins[i]).step;

        // Both axes to READY (same sequence as `ready_axis`).
        let enable = [word(cmd::ENABLE); 2];
        m.tick(0, &enable, &ins, DT, &mut outs);
        for input in ins.iter_mut() {
            input.drive = DriveStatus::Enabled;
        }
        m.tick(0, &enable, &ins, DT, &mut outs);
        assert_eq!(step_of(&m, 0, &ins), Step::Ready.as_i32());
        assert_eq!(step_of(&m, 1, &ins), Step::Ready.as_i32());

        // Axis 0 runs a MoveAbs to completion; its word stays latched.
        let go = AxisRequest {
            word: cmd::MOVE_ABS,
            move_abs_pos: 5.0,
            move_abs_vel: 50.0,
            fresh: true,
            ..Default::default()
        };
        let held = AxisRequest { fresh: false, ..go };
        let mut reqs = [go, word(0)];
        let mut done = false;
        for _ in 0..5000 {
            m.tick(0, &reqs, &ins, DT, &mut outs);
            for (out, input) in outs.iter().zip(ins.iter_mut()) {
                echo_drive(out, input);
            }
            reqs[0] = held;
            if step_of(&m, 0, &ins) == Step::Ready.as_i32() && ins[0].act_pos == 5.0 {
                done = true;
                break;
            }
        }
        assert!(done, "axis 0 move did not complete");

        // A new command message that only touches axis 1 (jog): axis 0's
        // latched MOVE_ABS bit must stay locked by its re-arm latch.
        let jog1 = AxisRequest {
            word: cmd::JOG_POS,
            jog_vel: 10.0,
            fresh: true,
            ..Default::default()
        };
        reqs = [held, jog1];
        for tick in 0..20 {
            m.tick(0, &reqs, &ins, DT, &mut outs);
            for (out, input) in outs.iter().zip(ins.iter_mut()) {
                echo_drive(out, input);
            }
            reqs[1].fresh = false;
            assert_eq!(
                step_of(&m, 0, &ins),
                Step::Ready.as_i32(),
                "tick {tick}: axis 1's fresh word re-armed axis 0"
            );
        }
        assert_eq!(step_of(&m, 1, &ins), Step::JogPos.as_i32());
        assert_eq!(ins[0].act_pos, 5.0);

        // A message that touches axis 0 too re-dispatches it.
        reqs[0] = AxisRequest { fresh: true, ..held };
        m.tick(0, &reqs, &ins, DT, &mut outs);
        assert_eq!(step_of(&m, 0, &ins), Step::MoveAbs.as_i32());
    }
}
