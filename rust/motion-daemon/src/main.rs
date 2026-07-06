//! motion-daemon: the CODESYS replacement. Owns the two /dev/shm segments
//! (`plc_data` writer, `plc_cmd` reader — the roles `PRG_ShmPublisher.st`
//! had), runs `motion-core` against a `fieldbus-api` backend, and pushes
//! axis state to the untouched Go bridge + Svelte HMI.
//!
//! Cycle order (EtherCAT-style: bus exchange at a fixed phase, compute
//! after): pace → exchange(prev outs) → latch command → tick state machines
//! → publish plc_data.

mod config;
mod rt;

use std::process::ExitCode;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use fieldbus_api::{AxisIn, AxisOut, BusState, DriveStatus, Fieldbus};
use motion_core::{AxisRequest, MachineControl};
use shm_bridge::{
    layout, trace, CmdReader, DataPublisher, Mapping, TraceSample, TraceWriter,
    SIZE_PLC_COMMAND, SIZE_PLC_DATA,
};

fn main() -> ExitCode {
    let cfg = match config::parse(std::env::args()) {
        Ok(cfg) => cfg,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };

    let res = match cfg.backend {
        config::Backend::Sim => {
            let sim = fieldbus_sim::SimBackend::new(fieldbus_sim::SimConfig {
                axes: cfg.axes,
                cycle: cfg.cycle,
                tau: cfg.sim_tau,
                ..Default::default()
            });
            run(sim, &cfg)
        }
        config::Backend::Ethercat => {
            if cfg.ifname.is_empty() {
                eprintln!("motion-daemon: the ethercat backend needs --ifname (or ifname= in the config)");
                return ExitCode::FAILURE;
            }
            let mut ecat = fieldbus_ethercat::EcatConfig::new(&cfg.ifname, 0);
            ecat.cycle = cfg.cycle;
            ecat.dc = cfg.ecat_dc;
            ecat.axes = (0..cfg.axes)
                .map(|i| fieldbus_ethercat::AxisMapping {
                    subdevice: cfg.first_axis_subdevice + i,
                    scale: cfg.scale,
                })
                .collect();
            // RT discipline for the cycle thread (kickoff §4). Best effort:
            // without root/CAP_SYS_NICE the daemon still runs, with worse
            // jitter — bring-up territory, not production.
            if let Err(e) = rt::lock_memory() {
                eprintln!("motion-daemon: mlockall: {e} (continuing unlocked)");
            }
            if let Err(e) = rt::set_fifo(80) {
                eprintln!("motion-daemon: SCHED_FIFO: {e} (continuing best-effort)");
            }
            run(fieldbus_ethercat::EthercatBackend::new(ecat), &cfg)
        }
    };

    match res {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("motion-daemon: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run<B: Fieldbus>(mut bus: B, cfg: &config::Config) -> Result<(), Box<dyn std::error::Error>> {
    smol::block_on(async {
        rt::install_shutdown_signals();

        bus.start().await?;
        for i in 0..bus.axis_count() {
            let cap = bus.capability(fieldbus_api::AxisId(i));
            eprintln!("motion-daemon: axis{i} capability: {cap:?}");
        }

        // Segment creation is the daemon's job now (CODESYS did this via
        // SysSharedMemoryCreate). Modes pre-grant what the bridge unit's
        // ExecStartPre chmod would widen: world-readable data, world-writable
        // command.
        let data_map = Mapping::create(layout::NAME_PLC_DATA, SIZE_PLC_DATA, 0o644)?;
        let cmd_map = Mapping::create(layout::NAME_PLC_COMMAND, SIZE_PLC_COMMAND, 0o666)?;
        eprintln!(
            "motion-daemon: created /dev/shm/{{{},{}}} ({} + {} B), backend {:?}, {} axes, cycle {:?}",
            layout::NAME_PLC_DATA,
            layout::NAME_PLC_COMMAND,
            SIZE_PLC_DATA,
            SIZE_PLC_COMMAND,
            cfg.backend,
            cfg.axes,
            cfg.cycle
        );

        // Trace ring: one fixed 256 B sample per cycle for the HMI's
        // watch/trace panels. Off the command path — losing it costs
        // diagnostics, never motion.
        let mut tracer = if cfg.trace_seconds > 0 {
            let period_ns = cfg.cycle.as_nanos() as u64;
            let capacity = trace::capacity_for(cfg.trace_seconds, period_ns);
            let map =
                Mapping::create(trace::NAME_PLC_TRACE, trace::segment_size(capacity), 0o644)?;
            let epoch = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0);
            eprintln!(
                "motion-daemon: created /dev/shm/{} ({} samples, {:.1} s window, {} B)",
                trace::NAME_PLC_TRACE,
                capacity,
                capacity as f64 * period_ns as f64 / 1e9,
                trace::segment_size(capacity),
            );
            Some(TraceWriter::new(map, capacity, period_ns, epoch))
        } else {
            None
        };
        let t_epoch = Instant::now(); // pairs with the header's epoch_unix_ns

        let mut publisher = DataPublisher::new(data_map);
        let mut cmd = CmdReader::new(cmd_map);
        let mut machine = MachineControl::new(cfg.axes, cfg.params);

        let n = cfg.axes;
        let dt = cfg.cycle.as_secs_f64();
        let mut ins = vec![AxisIn::default(); n];
        let mut outs = vec![AxisOut::default(); n];
        let mut reqs = vec![AxisRequest::default(); n];

        // plc_cmd starts zeroed (magic 0) until the bridge's first write;
        // log the valid/invalid edges once instead of every cycle.
        let mut cmd_ok = false;
        let mut exchange_err: Option<fieldbus_api::FieldbusError> = None;

        let mut next = Instant::now() + cfg.cycle;
        let mut last_wake: Option<Instant> = None;
        while rt::running() {
            smol::Timer::at(next).await;
            next += cfg.cycle;
            let wake = Instant::now();
            let period_ns = last_wake.map_or(0, |w| (wake - w).as_nanos() as u32);
            last_wake = Some(wake);

            // 1. bus exchange: publish last cycle's outputs, read fresh inputs
            let exchange_ok = match bus.exchange(&outs, &mut ins).await {
                Ok(_) => {
                    if exchange_err.take().is_some() {
                        eprintln!("motion-daemon: exchange recovered");
                    }
                    true
                }
                Err(e) => {
                    if exchange_err != Some(e) {
                        eprintln!("motion-daemon: exchange: {e}");
                        exchange_err = Some(e);
                    }
                    false // keep pacing; stale ins, no new outs
                }
            };
            let exchange_ns = wake.elapsed().as_nanos() as u32;

            let mut fresh = false;
            if exchange_ok {
                // 2. latch the HMI command (PRG_ShmPublisher semantics)
                fresh = match cmd.poll() {
                    Ok(f) => {
                        if !cmd_ok {
                            eprintln!("motion-daemon: plc_cmd writer connected");
                            cmd_ok = true;
                        }
                        f
                    }
                    Err(_) if !cmd_ok => false, // no writer yet — expected at boot
                    Err(e) => {
                        eprintln!("motion-daemon: plc_cmd read: {e}");
                        cmd_ok = false;
                        false
                    }
                };
                let c = *cmd.current();
                for i in 0..n {
                    let a = &c.machine.axes[i];
                    reqs[i] = AxisRequest {
                        word: a.control_flags,
                        jog_vel: a.jog_vel,
                        move_abs_pos: a.move_abs_pos,
                        move_abs_vel: a.move_abs_vel,
                        fresh,
                    };
                }

                // 3. state machines
                machine.tick(c.machine.control_flags, &reqs, &ins, dt, &mut outs);

                // 4. publish plc_data
                let d = &mut publisher.data;
                d.system.temperature = 25.0;
                d.system.status_flags = u32::from(bus.bus_state() == BusState::Op);
                d.system.alarm_flags = 0;
                for i in 0..n {
                    let st = machine.status(i, &ins[i]);
                    let a = &mut d.machine.axes[i];
                    a.act_pos = ins[i].act_pos;
                    a.act_vel = ins[i].act_vel;
                    a.set_pos = st.set_pos;
                    a.set_vel = st.set_vel;
                    a.step = st.step;
                    a.flags = st.flags;
                    a.error_id = st.error_id;
                }
                d.machine.run_state = MachineControl::run_state(c.machine.control_flags);
                d.machine.alarms = 0;
                d.production.n_production_state = c.production.n_production_state;
                publisher.publish();

                // 5. bus events → log
                while let Some(ev) = bus.poll_event() {
                    eprintln!("motion-daemon: bus event: {ev:?}");
                }
            }

            // 6. trace sample — every cycle, *including* exchange-error ones
            // (the fault instant is exactly what a trace is for). On error
            // cycles the machine state is the last published one and `ins`
            // is stale; status_bits bit0 marks them.
            if let Some(t) = tracer.as_mut() {
                let status_bits = u8::from(!exchange_ok)
                    | u8::from(fresh) << 1
                    | u8::from(cmd_ok) << 2;
                let s = trace_sample(
                    &publisher.data,
                    &ins,
                    &outs,
                    bus.bus_state(),
                    (wake - t_epoch).as_nanos() as u64,
                    period_ns,
                    exchange_ns,
                    status_bits,
                );
                t.push(&s);
            }
        }

        eprintln!("motion-daemon: shutting down");
        bus.stop().await?;
        Ok(())
    })
}

/// Build one trace sample from the state already in scope at the end of a
/// cycle. Encodings match `shm_bridge::trace`'s docs (shm-bridge stays
/// fieldbus-agnostic, so the enum→code mapping lives here).
#[allow(clippy::too_many_arguments)] // it's a record, not an API
fn trace_sample(
    d: &layout::PlcData,
    ins: &[AxisIn],
    outs: &[AxisOut],
    bus_state: BusState,
    t_mono_ns: u64,
    period_ns: u32,
    exchange_ns: u32,
    status_bits: u8,
) -> TraceSample {
    let mut s = TraceSample {
        cycle: d.header.cycle,
        t_mono_ns,
        period_ns,
        exchange_ns,
        bus_state: match bus_state {
            BusState::Init => 0,
            BusState::PreOp => 1,
            BusState::SafeOp => 2,
            BusState::Op => 3,
        },
        status_bits,
        _pad: 0,
        run_state: d.machine.run_state,
        axes: Default::default(),
    };
    for (i, input) in ins.iter().enumerate() {
        let st = &d.machine.axes[i];
        s.axes[i] = shm_bridge::TraceAxisSample {
            act_pos: input.act_pos,
            act_vel: input.act_vel,
            set_pos: st.set_pos,
            set_vel: st.set_vel,
            step: st.step,
            flags: st.flags,
            error_id: st.error_id,
            fault_code: input.fault_code,
            drive_status: match input.drive {
                DriveStatus::Offline => 0,
                DriveStatus::Disabled => 1,
                DriveStatus::Enabling => 2,
                DriveStatus::Enabled => 3,
                DriveStatus::QuickStop => 4,
                DriveStatus::Fault => 5,
            },
            io_bits: u32::from(input.pos_limit)
                | u32::from(input.neg_limit) << 1
                | u32::from(input.homed) << 2
                | u32::from(outs[i].enable) << 3
                | u32::from(outs[i].fault_reset) << 4,
        };
    }
    s
}
