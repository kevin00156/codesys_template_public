//! The per-cycle logic of the daemon, split from timing and bus I/O so unit
//! tests can drive it with in-memory `plc_cmd`/`plc_data` segments and the
//! sim backend. `run()` in main.rs only paces, calls the bus and traces; the
//! [`Engine`] sees the *result* of each exchange and owns everything that
//! turns it into published state: the command reader, the machine state
//! machines, the data publisher, the command dead-man and the shutdown
//! sequence.
//!
//! Cycle order (EtherCAT-style: bus exchange at a fixed phase, compute
//! after): exchange(prev outs) → latch command → tick state machines →
//! publish plc_data. Nothing on the [`Engine::cycle`] path allocates.

use std::time::{Duration, Instant};

use fieldbus_api::{AxisIn, AxisOut, BusState, ExchangeStatus, FieldbusError, Setpoint};
use motion_core::flags::{cmd, machine_cmd, status};
use motion_core::{AxisParams, AxisRequest, MachineControl};
use shm_bridge::{CmdReader, DataPublisher, Mapping, PlcData};

use crate::cmdflags;

const JOG_BITS: u32 = cmd::JOG_POS | cmd::JOG_NEG;

/// Phase 2 of the controlled stop: cycles exchanged with `enable = false`
/// before `bus.stop()` — enough for the 402 sequencer to walk Operation
/// enabled → Switched on → Ready to switch on with the drive still powered
/// and reporting, instead of the coast a bus drop would cause.
pub const SHUTDOWN_DISABLE_CYCLES: u32 = 10;

#[derive(Clone, Copy, Debug)]
pub struct EngineConfig {
    /// Served axes (1..=4).
    pub axes: usize,
    pub params: AxisParams,
    /// Command dead-man window; `None` disables it.
    pub cmd_timeout: Option<Duration>,
    /// Upper bound on the EMS-ramp phase of the controlled stop.
    pub shutdown_timeout: Duration,
}

/// What one [`Engine::cycle`] / [`Engine::shutdown_step`] did — the bits the
/// trace sample records.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CycleReport {
    /// The exchange this cycle was built on succeeded (state machines ticked).
    pub exchange_ok: bool,
    /// A new command message was latched this cycle.
    pub fresh: bool,
    /// `plc_cmd` has a valid writer (the last poll decoded).
    pub cmd_ok: bool,
    /// `ExchangeStatus::working_counter_ok` was false.
    pub wkc_error: bool,
    /// `PlcData.system.alarm_flags` as published this cycle.
    pub alarm_flags: u32,
}

/// Where the controlled stop stands *after* a [`Engine::shutdown_step`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShutdownPhase {
    /// Phase 1: EMS forced on every axis, waiting for standstill.
    Decel,
    /// Phase 2: drives told to disable ([`SHUTDOWN_DISABLE_CYCLES`]).
    Disable,
    /// This was the last step; the caller may `bus.stop()`.
    Done,
}

enum Shutdown {
    NotStarted,
    Decel { since: Instant },
    Disable { cycles_left: u32 },
    Done,
}

pub struct Engine {
    publisher: DataPublisher,
    cmd: CmdReader,
    machine: MachineControl,
    /// Per-axis requests handed to the machine — preallocated, rebuilt each
    /// cycle from the latched command.
    reqs: Vec<AxisRequest>,
    cmd_timeout: Option<Duration>,
    shutdown_timeout: Duration,
    /// Last instant `CmdReader::poll` latched a new message.
    last_fresh: Instant,
    /// `plc_cmd` decodes (a writer is connected). Logged on edges only.
    cmd_ok: bool,
    /// Dead-man tripped: jog bits are being stripped and alarm bit 1 is up.
    /// Cleared (re-armed) by the next fresh message.
    cmd_timeout_tripped: bool,
    /// Last exchange error; `Some` == bus fault (alarm bit 0). Kept as the
    /// error itself so a *different* failure logs again.
    exchange_err: Option<FieldbusError>,
    wkc_errors: u64,
    shutdown: Shutdown,
}

impl Engine {
    /// `now` seeds the dead-man clock (nothing can be latched before the
    /// first message anyway, but the arithmetic needs a base).
    pub fn new(cfg: EngineConfig, data_map: Mapping, cmd_map: Mapping, now: Instant) -> Engine {
        Engine {
            publisher: DataPublisher::new(data_map),
            cmd: CmdReader::new(cmd_map),
            machine: MachineControl::new(cfg.axes, cfg.params),
            reqs: vec![AxisRequest::default(); cfg.axes],
            cmd_timeout: cfg.cmd_timeout,
            shutdown_timeout: cfg.shutdown_timeout,
            last_fresh: now,
            cmd_ok: false,
            cmd_timeout_tripped: false,
            exchange_err: None,
            wkc_errors: 0,
            shutdown: Shutdown::NotStarted,
        }
    }

    /// The `plc_data` image as last published (the trace sample reads it).
    pub fn data(&self) -> &PlcData {
        &self.publisher.data
    }

    /// Exchanges whose bus-level integrity check failed, since start.
    pub fn wkc_errors(&self) -> u64 {
        self.wkc_errors
    }

    /// One normal cycle. `exchange` is the result of the bus call made with
    /// the previous `outs`; `ins` are its inputs (stale on `Err`).
    ///
    /// On a bus fault the state machines are *not* ticked (their inputs are
    /// stale) but the command is still polled — so an EMS or reset pressed
    /// during the outage is latched and acts the instant the bus is back —
    /// and `plc_data` is still published with the last axis status, alarm
    /// bit 0 and an advancing cycle counter, so the HMI sees "bus down"
    /// rather than the frozen counter of a dead daemon.
    pub fn cycle(
        &mut self,
        now: Instant,
        exchange: &Result<ExchangeStatus, FieldbusError>,
        ins: &[AxisIn],
        outs: &mut [AxisOut],
        bus_state: BusState,
        dt: f64,
    ) -> CycleReport {
        let (exchange_ok, wkc_error) = self.note_exchange(exchange);
        let fresh = self.poll_command(now);
        let machine_word = self.build_requests(now, fresh);
        if exchange_ok {
            self.machine.tick(machine_word, &self.reqs, ins, dt, outs);
        }
        self.publish(ins, bus_state, machine_word);
        CycleReport {
            exchange_ok,
            fresh,
            cmd_ok: self.cmd_ok,
            wkc_error,
            alarm_flags: self.publisher.data.system.alarm_flags,
        }
    }

    /// One cycle of the controlled stop (call instead of [`Engine::cycle`]
    /// once the shutdown signal arrived, keeping the bus exchange going).
    ///
    /// Phase 1 ticks the machine with EMS forced until every served axis
    /// reports STANDSTILL or `shutdown_timeout` elapses; phase 2 sends
    /// `enable = false` / `Hold` for [`SHUTDOWN_DISABLE_CYCLES`] cycles so
    /// the drives perform a controlled Shutdown transition. Only after
    /// `Done` should the caller release the bus — `bus.stop()` on a moving
    /// axis means a coast.
    pub fn shutdown_step(
        &mut self,
        now: Instant,
        exchange: &Result<ExchangeStatus, FieldbusError>,
        ins: &[AxisIn],
        outs: &mut [AxisOut],
        bus_state: BusState,
        dt: f64,
    ) -> (CycleReport, ShutdownPhase) {
        let (exchange_ok, wkc_error) = self.note_exchange(exchange);
        let fresh = self.poll_command(now);
        let machine_word = self.build_requests(now, fresh) | machine_cmd::EMS;

        if let Shutdown::NotStarted = self.shutdown {
            eprintln!(
                "motion-daemon: shutdown phase 1: EMS ramp on all axes (timeout {:?})",
                self.shutdown_timeout
            );
            self.shutdown = Shutdown::Decel { since: now };
        }

        let phase = match self.shutdown {
            Shutdown::Decel { since } => {
                if exchange_ok {
                    self.machine.tick(machine_word, &self.reqs, ins, dt, outs);
                }
                // Stale inputs cannot prove standstill; with the bus down
                // only the timeout leads on.
                let still = exchange_ok
                    && ins.iter().enumerate().all(|(i, input)| {
                        self.machine.status(i, input).flags & status::STANDSTILL != 0
                    });
                let timed_out = now.saturating_duration_since(since) >= self.shutdown_timeout;
                if still || timed_out {
                    eprintln!(
                        "motion-daemon: shutdown phase 2: disabling drives ({})",
                        if still {
                            "all axes at standstill"
                        } else {
                            "phase 1 timed out"
                        }
                    );
                    self.shutdown = Shutdown::Disable {
                        cycles_left: SHUTDOWN_DISABLE_CYCLES,
                    };
                    ShutdownPhase::Disable
                } else {
                    ShutdownPhase::Decel
                }
            }
            Shutdown::Disable { cycles_left } => {
                for out in outs.iter_mut() {
                    *out = AxisOut {
                        enable: false,
                        fault_reset: false,
                        setpoint: Setpoint::Hold,
                    };
                }
                let left = cycles_left.saturating_sub(1);
                if left == 0 {
                    eprintln!("motion-daemon: shutdown complete, releasing the bus");
                    self.shutdown = Shutdown::Done;
                    ShutdownPhase::Done
                } else {
                    self.shutdown = Shutdown::Disable { cycles_left: left };
                    ShutdownPhase::Disable
                }
            }
            Shutdown::Done => ShutdownPhase::Done,
            Shutdown::NotStarted => unreachable!("started above"),
        };

        self.publish(ins, bus_state, machine_word);
        let report = CycleReport {
            exchange_ok,
            fresh,
            cmd_ok: self.cmd_ok,
            wkc_error,
            alarm_flags: self.publisher.data.system.alarm_flags,
        };
        (report, phase)
    }

    /// Log exchange edges, track the bus-fault alarm and the WKC counter.
    /// Returns `(exchange_ok, wkc_error)`.
    fn note_exchange(&mut self, exchange: &Result<ExchangeStatus, FieldbusError>) -> (bool, bool) {
        match exchange {
            Ok(st) => {
                if self.exchange_err.take().is_some() {
                    eprintln!("motion-daemon: exchange recovered");
                }
                if !st.working_counter_ok {
                    self.wkc_errors += 1;
                }
                (true, !st.working_counter_ok)
            }
            Err(e) => {
                if self.exchange_err != Some(*e) {
                    eprintln!("motion-daemon: exchange: {e}");
                    self.exchange_err = Some(*e);
                }
                (false, false)
            }
        }
    }

    /// Latch the HMI command (PRG_ShmPublisher semantics). Returns whether a
    /// new message arrived this cycle; a fresh message also re-arms the
    /// dead-man.
    fn poll_command(&mut self, now: Instant) -> bool {
        // plc_cmd starts zeroed (magic 0) until the bridge's first write;
        // log the valid/invalid edges once instead of every cycle.
        let fresh = match self.cmd.poll() {
            Ok(f) => {
                if !self.cmd_ok {
                    eprintln!("motion-daemon: plc_cmd writer connected");
                    self.cmd_ok = true;
                }
                f
            }
            Err(_) if !self.cmd_ok => false, // no writer yet — expected at boot
            Err(e) => {
                eprintln!("motion-daemon: plc_cmd read: {e}");
                self.cmd_ok = false;
                false
            }
        };
        if fresh {
            self.last_fresh = now;
            if self.cmd_timeout_tripped {
                eprintln!("motion-daemon: command dead-man re-armed (new message)");
                self.cmd_timeout_tripped = false;
            }
        }
        fresh
    }

    /// Rebuild `reqs` from the latched command: per-axis fresh edges from
    /// the header touch mask, and the dead-man strip. Returns the machine
    /// word.
    ///
    /// The bridge's own jog watchdog covers a vanished *client*; this covers
    /// a vanished *bridge* — plc_bridge killed with a jog word latched would
    /// otherwise keep the axis moving forever. Only the jog bits go: EMS and
    /// the other level-held bits must stay in force.
    fn build_requests(&mut self, now: Instant, fresh: bool) -> u32 {
        let c = self.cmd.current();
        let flags = c.header.flags;
        let stale = self
            .cmd_timeout
            .is_some_and(|t| now.saturating_duration_since(self.last_fresh) > t);
        let mut stripped = false;
        for (i, (req, a)) in self.reqs.iter_mut().zip(&c.machine.axes).enumerate() {
            let mut word = a.control_flags;
            if stale && word & JOG_BITS != 0 {
                word &= !JOG_BITS;
                stripped = true;
            }
            *req = AxisRequest {
                word,
                jog_vel: a.jog_vel,
                move_abs_pos: a.move_abs_pos,
                move_abs_vel: a.move_abs_vel,
                fresh: cmdflags::axis_fresh(fresh, flags, i),
            };
        }
        if stripped && !self.cmd_timeout_tripped {
            eprintln!(
                "motion-daemon: command dead-man tripped: no message for {:?}, jog bits stripped",
                now.saturating_duration_since(self.last_fresh)
            );
            self.cmd_timeout_tripped = true;
        }
        c.machine.control_flags
    }

    fn alarm_flags(&self) -> u32 {
        let mut alarms = 0;
        if self.exchange_err.is_some() {
            alarms |= cmdflags::ALARM_BUS_FAULT;
        }
        if self.cmd_timeout_tripped {
            alarms |= cmdflags::ALARM_CMD_TIMEOUT;
        }
        alarms
    }

    /// Fill and publish `plc_data`. Slots beyond the served axes keep their
    /// zeros.
    fn publish(&mut self, ins: &[AxisIn], bus_state: BusState, machine_word: u32) {
        let alarms = self.alarm_flags();
        let c = self.cmd.current();
        let d = &mut self.publisher.data;
        d.system.temperature = 25.0;
        d.system.status_flags = if bus_state == BusState::Op {
            cmdflags::STATUS_BUS_OP
        } else {
            0
        };
        d.system.alarm_flags = alarms;
        for (i, (a, input)) in d.machine.axes.iter_mut().zip(ins).enumerate() {
            let st = self.machine.status(i, input);
            a.act_pos = input.act_pos;
            a.act_vel = input.act_vel;
            a.set_pos = st.set_pos;
            a.set_vel = st.set_vel;
            a.step = st.step;
            a.flags = st.flags;
            a.error_id = st.error_id;
        }
        d.machine.run_state = MachineControl::run_state(machine_word);
        d.machine.alarms = 0;
        d.production.n_production_state = c.production.n_production_state;
        self.publisher.publish();
    }
}

// ─── Cycle pacing ────────────────────────────────────────────────────────────

/// Where the next deadline goes, decided from one wake.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pacing {
    pub next: Instant,
    /// The wake came more than a full cycle after the deadline it was
    /// scheduled for — at least one whole deadline was missed.
    pub overrun: bool,
}

/// Pure pacing arithmetic for the cycle loop. `deadline` is what this wake
/// was scheduled for, `wake` the instant taken right before `exchange`,
/// `dc_wait` the bus's `ExchangeStatus::next_cycle_wait`.
///
/// * On time, or late by less than a cycle: `next = deadline + cycle`.
///   Absolute-time pacing absorbs the lateness without drift.
/// * Overrun (`wake > deadline + cycle`): resync to `wake + cycle`. Letting
///   `Timer::at` fire back-to-back to "catch up" after a stall bursts
///   compressed setpoints at the drives — one dropped cycle is the lesser
///   evil.
/// * DC phase lock (`dc_wait = Some(w)`): `next = wake + w`, overrun or not.
///   `w` is measured from the exchange call (ethercrab's convention); the
///   bus clock, not the host timer, is the reference.
pub fn next_deadline(
    deadline: Instant,
    wake: Instant,
    cycle: Duration,
    dc_wait: Option<Duration>,
) -> Pacing {
    let overrun = wake > deadline + cycle;
    let next = match dc_wait {
        Some(w) => wake + w,
        None if overrun => wake + cycle,
        None => deadline + cycle,
    };
    Pacing { next, overrun }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::ptr::copy_nonoverlapping;
    use fieldbus_api::{DriveStatus, Fieldbus};
    use fieldbus_sim::{SimBackend, SimConfig};
    use motion_core::Step;
    use shm_bridge::{
        AxisState, PlcCommand, PLC_COMMAND_MAGIC, PLC_COMMAND_VERSION, SIZE_PLC_COMMAND,
        SIZE_PLC_DATA,
    };

    const CYCLE: Duration = Duration::from_millis(2);
    const DT: f64 = 0.002;

    /// Test-side writer for the engine's `plc_cmd`. `CmdReader` owns its
    /// `Mapping`, so the test keeps the raw pointer it took before the
    /// hand-over and copies a `shm_bridge::publish`ed image over it between
    /// cycles — single-threaded, no reader runs during the copy (the same
    /// trick shm-bridge's own tests use to seed a segment). Like the Go
    /// sink, the whole struct persists between messages: an edit touching
    /// one axis still ships the others' latched words.
    struct CmdWriter {
        shadow: Mapping,
        dst: *mut u8,
        msg: PlcCommand,
    }

    impl CmdWriter {
        fn send(&mut self, edit: impl FnOnce(&mut PlcCommand)) {
            self.msg.header.flags = 0; // legacy writer unless the edit sets a mask
            edit(&mut self.msg);
            self.msg.header.magic = PLC_COMMAND_MAGIC;
            self.msg.header.version = PLC_COMMAND_VERSION;
            self.msg.header.cycle += 1;
            shm_bridge::publish(&self.shadow, &self.msg);
            // Safety: dst points into the engine's live in-memory mapping
            // (kept alive by the Engine the Rig owns), same length, and no
            // reader is active between cycles.
            unsafe { copy_nonoverlapping(self.shadow.ptr(), self.dst, SIZE_PLC_COMMAND) };
        }
    }

    struct Rig {
        engine: Engine,
        bus: SimBackend,
        ins: Vec<AxisIn>,
        outs: Vec<AxisOut>,
        now: Instant,
        writer: CmdWriter,
        data_ptr: *mut u8,
    }

    impl Rig {
        fn new(axes: usize, cmd_timeout: Option<Duration>) -> Rig {
            let mut bus = SimBackend::new(SimConfig {
                axes,
                cycle: CYCLE,
                ..Default::default()
            });
            smol::block_on(bus.start()).unwrap();
            let data_map = Mapping::in_memory(SIZE_PLC_DATA);
            let cmd_map = Mapping::in_memory(SIZE_PLC_COMMAND);
            let data_ptr = data_map.ptr();
            let dst = cmd_map.ptr();
            let now = Instant::now();
            let engine = Engine::new(
                EngineConfig {
                    axes,
                    params: AxisParams {
                        max_vel: 100.0,
                        acc: 1000.0,
                        dec: 1000.0,
                        stop_dec: 2000.0,
                    },
                    cmd_timeout,
                    shutdown_timeout: Duration::from_secs(2),
                },
                data_map,
                cmd_map,
                now,
            );
            Rig {
                engine,
                bus,
                ins: vec![AxisIn::default(); axes],
                outs: vec![AxisOut::default(); axes],
                now,
                writer: CmdWriter {
                    shadow: Mapping::in_memory(SIZE_PLC_COMMAND),
                    dst,
                    msg: PlcCommand::default(),
                },
                data_ptr,
            }
        }

        fn exchange(&mut self) -> Result<ExchangeStatus, FieldbusError> {
            self.now += CYCLE;
            smol::block_on(self.bus.exchange(&self.outs, &mut self.ins))
        }

        fn step(&mut self) -> CycleReport {
            let ex = self.exchange();
            let state = self.bus.bus_state();
            self.engine
                .cycle(self.now, &ex, &self.ins, &mut self.outs, state, DT)
        }

        fn shutdown_step(&mut self) -> (CycleReport, ShutdownPhase) {
            let ex = self.exchange();
            let state = self.bus.bus_state();
            self.engine
                .shutdown_step(self.now, &ex, &self.ins, &mut self.outs, state, DT)
        }

        fn run(&mut self, cycles: usize) {
            for _ in 0..cycles {
                self.step();
            }
        }

        fn run_until(&mut self, max: usize, pred: impl Fn(&PlcData) -> bool) {
            for _ in 0..max {
                self.step();
                if pred(self.engine.data()) {
                    return;
                }
            }
            panic!(
                "condition not reached; axes: {:?}",
                self.engine.data().machine.axes
            );
        }

        fn axis(&self, i: usize) -> &AxisState {
            &self.engine.data().machine.axes[i]
        }

        fn send(&mut self, edit: impl FnOnce(&mut PlcCommand)) {
            self.writer.send(edit);
        }

        /// Read the real segment (proves the image went through the seqlock).
        fn snapshot(&self) -> PlcData {
            let shadow = Mapping::in_memory(SIZE_PLC_DATA);
            // Safety: data_ptr points into the engine's live mapping; the
            // engine is idle between cycles.
            unsafe { copy_nonoverlapping(self.data_ptr, shadow.ptr(), SIZE_PLC_DATA) };
            let mut d = PlcData::default();
            shm_bridge::snapshot(&shadow, &mut d).expect("published image decodes");
            d
        }

        fn enable_all(&mut self) {
            self.send(|c| {
                for a in c.machine.axes.iter_mut() {
                    a.control_flags = cmd::ENABLE;
                }
            });
            let n = self.ins.len();
            self.run_until(50, |d| {
                d.machine.axes[..n]
                    .iter()
                    .all(|a| a.step == Step::Ready.as_i32())
            });
        }

        fn jog(&mut self, axis: usize) {
            self.send(|c| {
                c.machine.axes[axis].control_flags = cmd::JOG_POS;
                c.machine.axes[axis].jog_vel = 10.0;
            });
            self.run_until(500, |d| {
                d.machine.axes[axis].step == Step::JogPos.as_i32() && d.machine.axes[axis].act_vel > 5.0
            });
        }
    }

    // (a) enable + jog → plc_data shows JOG_POS, act_vel > 0, counter advancing.
    #[test]
    fn enable_and_jog_publish_state() {
        let mut rig = Rig::new(1, None);
        // Before any writer: publishing already, nothing latched.
        let r = rig.step();
        assert!(r.exchange_ok && !r.cmd_ok && !r.fresh);
        assert_eq!(rig.axis(0).step, Step::Idle.as_i32());

        rig.enable_all();
        let c0 = rig.engine.data().header.cycle;
        rig.jog(0);
        let c1 = rig.engine.data().header.cycle;
        assert!(c1 > c0, "cycle counter must advance");

        let before = rig.engine.data().header.cycle;
        for k in 1..=10u64 {
            let r = rig.step();
            assert!(r.cmd_ok && r.exchange_ok);
            assert_eq!(rig.engine.data().header.cycle, before + k);
        }
        let a = rig.axis(0);
        assert_eq!(a.step, Step::JogPos.as_i32());
        assert!(a.act_vel > 5.0 && a.act_pos > 0.0, "{a:?}");
        assert!(a.flags & status::ENABLED != 0 && a.flags & status::BUSY != 0);
        assert_eq!(rig.engine.data().system.status_flags, cmdflags::STATUS_BUS_OP);
        assert_eq!(rig.engine.data().system.alarm_flags, 0);

        // The segment carries exactly the staged image.
        let seg = rig.snapshot();
        assert_eq!(seg.header.cycle, rig.engine.data().header.cycle);
        assert_eq!(seg.machine.axes[0], *rig.axis(0));
    }

    // (b) per-axis fresh via the header touch mask.
    #[test]
    fn touch_mask_scopes_fresh_edges_per_axis() {
        let mut rig = Rig::new(2, None);
        rig.enable_all();

        // Axis 0 MoveAbs, touch mask names axis 0 only.
        rig.send(|c| {
            c.header.flags = cmdflags::TOUCH_MASK_PRESENT | cmdflags::touch_bit(0);
            c.machine.axes[0].control_flags = cmd::MOVE_ABS;
            c.machine.axes[0].move_abs_pos = 5.0;
            c.machine.axes[0].move_abs_vel = 50.0;
        });
        rig.run_until(10, |d| d.machine.axes[0].step == Step::MoveAbs.as_i32());
        rig.run_until(2000, |d| d.machine.axes[0].step == Step::Ready.as_i32());
        assert!((rig.axis(0).act_pos - 5.0).abs() < 0.05, "{:?}", rig.axis(0));
        assert_eq!(rig.axis(1).step, Step::Ready.as_i32());

        // A message naming only axis 1 ships axis 0's still-latched MOVE_ABS
        // word but must not re-dispatch it.
        rig.send(|c| {
            c.header.flags = cmdflags::TOUCH_MASK_PRESENT | cmdflags::touch_bit(1);
            c.machine.axes[1].control_flags = cmd::ENABLE;
        });
        for _ in 0..100 {
            rig.step();
            assert_eq!(rig.axis(0).step, Step::Ready.as_i32(), "axis 0 re-dispatched");
        }

        // Naming axis 0 again re-dispatches (new target).
        rig.send(|c| {
            c.header.flags = cmdflags::TOUCH_MASK_PRESENT | cmdflags::touch_bit(0);
            c.machine.axes[0].move_abs_pos = -2.0;
        });
        rig.run_until(10, |d| d.machine.axes[0].step == Step::MoveAbs.as_i32());
        rig.run_until(2000, |d| d.machine.axes[0].step == Step::Ready.as_i32());
        assert!((rig.axis(0).act_pos + 2.0).abs() < 0.05, "{:?}", rig.axis(0));

        // Legacy writer (no marker): every axis is fresh → re-dispatch.
        rig.send(|c| c.machine.axes[0].move_abs_pos = 1.0);
        rig.run_until(10, |d| d.machine.axes[0].step == Step::MoveAbs.as_i32());
    }

    // (c) command dead-man strips jog bits after cmd_timeout, alarm bit 1,
    // re-armed by the next message.
    #[test]
    fn dead_man_strips_jog_and_rearms() {
        let mut rig = Rig::new(1, Some(Duration::from_secs(1)));
        rig.enable_all();
        rig.jog(0);
        assert_eq!(rig.engine.data().system.alarm_flags, 0);

        // Bridge dies: no more messages. Jump the clock past the window.
        rig.now += Duration::from_millis(1500);
        let r = rig.step();
        assert_eq!(r.alarm_flags, cmdflags::ALARM_CMD_TIMEOUT);
        assert_eq!(rig.axis(0).step, Step::Stopping.as_i32(), "jog bits stripped");
        rig.run_until(1000, |d| d.machine.axes[0].step == Step::Ready.as_i32());
        assert!(rig.axis(0).act_vel.abs() < 1e-3);
        assert_eq!(
            rig.engine.data().system.alarm_flags,
            cmdflags::ALARM_CMD_TIMEOUT,
            "alarm holds until a message arrives"
        );
        assert!(rig.axis(0).flags & status::ENABLED != 0, "power stays on");

        // Bridge back: a fresh message clears the alarm and jog runs again.
        rig.send(|c| c.machine.axes[0].control_flags = cmd::JOG_POS);
        let r = rig.step();
        assert!(r.fresh);
        assert_eq!(r.alarm_flags, 0);
        rig.run_until(500, |d| d.machine.axes[0].act_vel > 5.0);
        assert_eq!(rig.axis(0).step, Step::JogPos.as_i32());
    }

    // Dead-man must leave level-held bits (EMS) alone.
    #[test]
    fn dead_man_keeps_ems_in_force() {
        let mut rig = Rig::new(1, Some(Duration::from_secs(1)));
        rig.enable_all();
        rig.jog(0);
        rig.send(|c| c.machine.control_flags = machine_cmd::EMS);
        rig.run_until(10, |d| d.machine.axes[0].step == Step::Stopping.as_i32());
        rig.now += Duration::from_secs(2);
        rig.run(600);
        assert_eq!(rig.axis(0).step, Step::Stopping.as_i32(), "EMS still freezes the machine");
        assert!(rig.axis(0).act_vel.abs() < 1e-3);
    }

    // (d) exchange Err → still publishing with alarm bit 0; EMS latched during
    // the outage acts on recovery.
    #[test]
    fn bus_fault_keeps_publishing_and_latches_commands() {
        let mut rig = Rig::new(1, None);
        rig.enable_all();
        rig.jog(0);
        let jog_pos = rig.axis(0).act_pos;

        smol::block_on(rig.bus.stop()).unwrap(); // exchange now errors
        let before = rig.engine.data().header.cycle;
        for k in 1..=5u64 {
            let r = rig.step();
            assert!(!r.exchange_ok);
            assert_eq!(r.alarm_flags, cmdflags::ALARM_BUS_FAULT);
            let d = rig.engine.data();
            assert_eq!(d.header.cycle, before + k, "counter must advance through the fault");
            assert_eq!(d.system.status_flags, 0, "bus not in OP");
            assert_eq!(d.machine.axes[0].step, Step::JogPos.as_i32(), "last status, no tick");
            assert_eq!(d.machine.axes[0].act_pos, jog_pos, "inputs frozen");
        }
        let seg = rig.snapshot();
        assert_eq!(seg.system.alarm_flags, cmdflags::ALARM_BUS_FAULT);

        // EMS pressed while the bus is down: latched, not yet acted on.
        rig.send(|c| c.machine.control_flags = machine_cmd::EMS);
        let r = rig.step();
        assert!(r.fresh && !r.exchange_ok);
        assert_eq!(rig.axis(0).step, Step::JogPos.as_i32());

        smol::block_on(rig.bus.start()).unwrap();
        let r = rig.step();
        assert!(r.exchange_ok);
        assert_eq!(r.alarm_flags, 0, "bus-fault alarm clears on recovery");
        assert_eq!(rig.axis(0).step, Step::Stopping.as_i32(), "EMS took effect");
        rig.run(600);
        assert_eq!(rig.axis(0).step, Step::Stopping.as_i32());
        assert!(rig.axis(0).act_vel.abs() < 1e-3, "{:?}", rig.axis(0));
        assert_eq!(rig.engine.data().system.status_flags, cmdflags::STATUS_BUS_OP);
    }

    // WKC failures are counted and reported, the machine still ticks.
    #[test]
    fn wkc_error_is_reported_not_fatal() {
        let mut rig = Rig::new(1, None);
        rig.enable_all();
        let ex = Ok(ExchangeStatus {
            all_axes_responding: true,
            inputs_fresh: true,
            working_counter_ok: false,
            next_cycle_wait: None,
        });
        let state = rig.bus.bus_state();
        let r = rig
            .engine
            .cycle(rig.now, &ex, &rig.ins, &mut rig.outs, state, DT);
        assert!(r.exchange_ok && r.wkc_error);
        assert_eq!(r.alarm_flags, 0);
        assert_eq!(rig.engine.wkc_errors(), 1);
    }

    // (e) pacing arithmetic.
    #[test]
    fn pacing_on_time_and_small_lateness_keep_absolute_schedule() {
        let t0 = Instant::now();
        let cycle = Duration::from_millis(2);
        let p = next_deadline(t0, t0 + Duration::from_micros(50), cycle, None);
        assert!(!p.overrun);
        assert_eq!(p.next, t0 + cycle);
        // Late by less than a cycle: absorbed, no resync.
        let p = next_deadline(t0, t0 + Duration::from_micros(1900), cycle, None);
        assert!(!p.overrun);
        assert_eq!(p.next, t0 + cycle);
        // Exactly one cycle late is the boundary — not yet an overrun.
        let p = next_deadline(t0, t0 + cycle, cycle, None);
        assert!(!p.overrun);
        assert_eq!(p.next, t0 + cycle);
    }

    #[test]
    fn pacing_overrun_resyncs_instead_of_bursting() {
        let t0 = Instant::now();
        let cycle = Duration::from_millis(2);
        let wake = t0 + Duration::from_millis(7); // 3.5 cycles late (stall)
        let p = next_deadline(t0, wake, cycle, None);
        assert!(p.overrun);
        assert_eq!(p.next, wake + cycle, "next deadline is a full cycle away");
        assert!(p.next > t0 + cycle, "never schedules into the past");
    }

    #[test]
    fn pacing_dc_wait_wins() {
        let t0 = Instant::now();
        let cycle = Duration::from_millis(2);
        let wake = t0 + Duration::from_micros(30);
        let dc = Duration::from_micros(1970);
        let p = next_deadline(t0, wake, cycle, Some(dc));
        assert!(!p.overrun);
        assert_eq!(p.next, wake + dc);
        // Overrun with DC: still counted, still phase-locked to the bus.
        let late = t0 + Duration::from_millis(9);
        let p = next_deadline(t0, late, cycle, Some(dc));
        assert!(p.overrun);
        assert_eq!(p.next, late + dc);
    }

    // (f) controlled stop: EMS ramp to standstill, then disable.
    #[test]
    fn shutdown_ramps_then_disables() {
        let mut rig = Rig::new(2, None);
        rig.enable_all();
        rig.jog(0);
        rig.send(|c| {
            c.machine.axes[1].control_flags = cmd::JOG_NEG;
            c.machine.axes[1].jog_vel = 20.0;
        });
        rig.run_until(500, |d| d.machine.axes[1].act_vel < -10.0);

        let start = rig.now;
        let mut decel = 0;
        let mut disabled = 0;
        loop {
            let (_, phase) = rig.shutdown_step();
            match phase {
                ShutdownPhase::Decel => {
                    decel += 1;
                    assert!(decel < 2000, "phase 1 never ended");
                }
                ShutdownPhase::Disable | ShutdownPhase::Done => {
                    if disabled == 0 {
                        // Transition step: reached by standstill, well inside the timeout.
                        assert!(decel > 10, "must actually ramp");
                        assert!(rig.now - start < Duration::from_secs(2), "not the timeout path");
                        for (i, input) in rig.ins.iter().enumerate() {
                            assert!(input.act_vel.abs() < 1e-3, "axis{i} still moving: {input:?}");
                        }
                        assert!(rig.outs.iter().all(|o| o.enable), "power held through the ramp");
                    } else {
                        assert!(rig.outs.iter().all(|o| !o.enable && o.setpoint == Setpoint::Hold));
                    }
                    disabled += 1;
                    if phase == ShutdownPhase::Done {
                        break;
                    }
                }
            }
        }
        // Transition step + SHUTDOWN_DISABLE_CYCLES disable steps.
        assert_eq!(disabled, 1 + SHUTDOWN_DISABLE_CYCLES as usize);
        assert!(rig.outs.iter().all(|o| !o.enable));
        for (i, input) in rig.ins.iter().enumerate() {
            assert_eq!(input.drive, DriveStatus::Disabled, "axis{i}: {input:?}");
        }
        // plc_data kept publishing through the stop.
        let a = rig.axis(0);
        assert_eq!(a.step, Step::Stopping.as_i32());
        assert!(a.flags & status::ENABLED == 0);
    }

    // Bus down during shutdown: phase 1 cannot see standstill and must give
    // up after shutdown_timeout, never hang.
    #[test]
    fn shutdown_times_out_when_bus_is_down() {
        let mut rig = Rig::new(1, None);
        rig.enable_all();
        rig.jog(0);
        smol::block_on(rig.bus.stop()).unwrap();

        let start = rig.now;
        let mut steps = 0;
        loop {
            let (r, phase) = rig.shutdown_step();
            steps += 1;
            assert!(!r.exchange_ok);
            assert_eq!(r.alarm_flags, cmdflags::ALARM_BUS_FAULT);
            assert!(steps < 1100, "stuck");
            if phase == ShutdownPhase::Done {
                break;
            }
        }
        let elapsed = rig.now - start;
        assert!(elapsed >= Duration::from_secs(2), "{elapsed:?}");
        assert!(elapsed < Duration::from_millis(2100), "{elapsed:?}");
        assert!(rig.outs.iter().all(|o| !o.enable));
    }
}
