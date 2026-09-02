//! motion-daemon: the CODESYS replacement. Owns the two /dev/shm segments
//! (`plc_data` writer, `plc_cmd` reader — the roles `PRG_ShmPublisher.st`
//! had), runs `motion-core` against a `fieldbus-api` backend, and pushes
//! axis state to the untouched Go bridge + Svelte HMI.
//!
//! `run()` is timing, segment ownership and the bus calls; everything that
//! happens *between* two exchanges — command latch, state machines, publish,
//! dead-man, controlled stop — lives in [`engine::Engine`] so it can be unit
//! tested without a clock or a bus.

mod cmdflags;
mod config;
mod engine;
mod rt;

use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fieldbus_api::{AxisIn, AxisOut, BusState, DriveStatus, Fieldbus};
use shm_bridge::{
    layout, trace, Mapping, PlcCommand, PlcData, Segment, TraceSample, TraceWriter,
};

use engine::{Engine, EngineConfig, ShutdownPhase};

/// `TraceSample.status_bits`. b0..b2 are documented in `shm_bridge::trace`;
/// b3/b4 are new here and mirrored by the Go trace decoder.
mod status_bits {
    pub const EXCHANGE_ERROR: u8 = 1 << 0;
    pub const CMD_FRESH: u8 = 1 << 1;
    pub const CMD_VALID: u8 = 1 << 2;
    /// This wake missed at least one whole deadline (pacing resynced).
    pub const OVERRUN: u8 = 1 << 3;
    /// `ExchangeStatus::working_counter_ok` was false.
    pub const WKC_ERROR: u8 = 1 << 4;
}

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

/// Open one of the seqlock segments the way a daemon restart needs it.
///
/// `open_or_create` keeps the inode: the Go bridge maps `plc_data`/`plc_cmd`
/// once at start-up and never re-opens them, so an unlink-and-recreate
/// would leave it on an orphaned page forever (frozen data, commands that
/// never arrive). `reset` then zeroes the payload *under the seqlock
/// protocol* — a bridge reading mid-reset sees a clean `MagicMismatch`
/// ("PLC not connected"), never a torn old/zero mix, and a stale command
/// word from the previous life cannot be latched. Modes pre-grant what the
/// bridge unit's ExecStartPre chmod would widen (0644 data, 0666 command).
fn open_segment<T: Segment>(mode: u32) -> std::io::Result<Mapping> {
    let size = core::mem::size_of::<T>();
    let existed = std::path::Path::new("/dev/shm").join(T::NAME).exists();
    let map = Mapping::open_or_create(T::NAME, size, mode)?;
    shm_bridge::reset::<T>(&map);
    eprintln!(
        "motion-daemon: {} /dev/shm/{} ({size} B, mode {mode:o}){}",
        if existed { "reused" } else { "created" },
        T::NAME,
        if existed { ", payload reset" } else { "" },
    );
    Ok(map)
}

fn run<B: Fieldbus>(mut bus: B, cfg: &config::Config) -> Result<(), Box<dyn std::error::Error>> {
    smol::block_on(async {
        rt::install_shutdown_signals();

        bus.start().await?;
        for i in 0..bus.axis_count() {
            let cap = bus.capability(fieldbus_api::AxisId(i));
            eprintln!("motion-daemon: axis{i} capability: {cap:?}");
        }

        // Segment ownership is the daemon's job now (CODESYS did this via
        // SysSharedMemoryCreate).
        let data_map = open_segment::<PlcData>(0o644)?;
        let cmd_map = open_segment::<PlcCommand>(0o666)?;
        eprintln!(
            "motion-daemon: backend {:?}, {} axes, cycle {:?}, cmd dead-man {:?}, shutdown timeout {:?}",
            cfg.backend, cfg.axes, cfg.cycle, cfg.cmd_timeout, cfg.shutdown_timeout
        );

        // Trace ring: one fixed 256 B sample per cycle for the HMI's
        // watch/trace panels. Off the command path — losing it costs
        // diagnostics, never motion. Created fresh (unlike the seqlock
        // segments): its Go reader remaps on inode change and wants zero
        // pages plus a new epoch.
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

        let mut engine = Engine::new(
            EngineConfig {
                axes: cfg.axes,
                params: cfg.params,
                cmd_timeout: cfg.cmd_timeout,
                shutdown_timeout: cfg.shutdown_timeout,
            },
            data_map,
            cmd_map,
            Instant::now(),
        );

        let n = cfg.axes;
        let dt = cfg.cycle.as_secs_f64();
        let mut ins = vec![AxisIn::default(); n];
        let mut outs = vec![AxisOut::default(); n];

        let mut overruns: u64 = 0;
        let mut last_overrun_log: Option<Instant> = None;
        let mut next = Instant::now() + cfg.cycle;
        let mut last_wake: Option<Instant> = None;
        loop {
            smol::Timer::at(next).await;
            let wake = Instant::now();
            let period_ns = last_wake.map_or(0, |w| (wake - w).as_nanos() as u32);
            last_wake = Some(wake);

            // 1. bus exchange: publish last cycle's outputs, read fresh inputs
            let exchange = bus.exchange(&outs, &mut ins).await;
            let exchange_ns = wake.elapsed().as_nanos() as u32;

            // 2. pace the next wake: DC phase hint from the bus if it has
            // one, overrun resync otherwise (see engine::next_deadline).
            let dc_wait = exchange.as_ref().ok().and_then(|s| s.next_cycle_wait);
            let deadline = next;
            let pace = engine::next_deadline(deadline, wake, cfg.cycle, dc_wait);
            next = pace.next;
            if pace.overrun {
                overruns += 1;
                // Rate-limited: a stalled box would otherwise log per cycle.
                if last_overrun_log.map_or(true, |t| wake - t >= Duration::from_secs(1)) {
                    eprintln!(
                        "motion-daemon: cycle overrun #{overruns}: woke {:?} late, resynced",
                        wake.saturating_duration_since(deadline)
                    );
                    last_overrun_log = Some(wake);
                }
            }

            // 3. command latch → state machines → publish (or the controlled
            // stop once the shutdown signal arrived)
            let bus_state = bus.bus_state();
            let (report, done) = if rt::running() {
                (engine.cycle(wake, &exchange, &ins, &mut outs, bus_state, dt), false)
            } else if rt::force_quit() {
                eprintln!("motion-daemon: second signal, skipping the controlled stop");
                break;
            } else {
                let (r, phase) =
                    engine.shutdown_step(wake, &exchange, &ins, &mut outs, bus_state, dt);
                (r, phase == ShutdownPhase::Done)
            };

            // 4. bus events → log
            while let Some(ev) = bus.poll_event() {
                eprintln!("motion-daemon: bus event: {ev:?}");
            }

            // 5. trace sample — every cycle, *including* exchange-error ones
            // (the fault instant is exactly what a trace is for). On error
            // cycles the machine state is the last published one and `ins`
            // is stale; status_bits bit0 marks them.
            if let Some(t) = tracer.as_mut() {
                let mut bits = 0u8;
                if !report.exchange_ok {
                    bits |= status_bits::EXCHANGE_ERROR;
                }
                if report.fresh {
                    bits |= status_bits::CMD_FRESH;
                }
                if report.cmd_ok {
                    bits |= status_bits::CMD_VALID;
                }
                if pace.overrun {
                    bits |= status_bits::OVERRUN;
                }
                if report.wkc_error {
                    bits |= status_bits::WKC_ERROR;
                }
                let s = trace_sample(
                    engine.data(),
                    &ins,
                    &outs,
                    bus_state,
                    (wake - t_epoch).as_nanos() as u64,
                    period_ns,
                    exchange_ns,
                    bits,
                );
                t.push(&s);
            }

            if done {
                break;
            }
        }

        eprintln!(
            "motion-daemon: stopping bus ({overruns} overruns, {} WKC errors)",
            engine.wkc_errors()
        );
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
    for (((sample, st), input), out) in s
        .axes
        .iter_mut()
        .zip(&d.machine.axes)
        .zip(ins)
        .zip(outs)
    {
        *sample = shm_bridge::TraceAxisSample {
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
                | u32::from(out.enable) << 3
                | u32::from(out.fault_reset) << 4,
        };
    }
    s
}
