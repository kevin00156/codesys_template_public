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
use std::time::Instant;

use fieldbus_api::{AxisIn, AxisOut, BusState, Fieldbus};
use motion_core::{AxisRequest, MachineControl};
use shm_bridge::{
    layout, CmdReader, DataPublisher, Mapping, SIZE_PLC_COMMAND, SIZE_PLC_DATA,
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
        while rt::running() {
            smol::Timer::at(next).await;
            next += cfg.cycle;

            // 1. bus exchange: publish last cycle's outputs, read fresh inputs
            match bus.exchange(&outs, &mut ins).await {
                Ok(_) => {
                    if exchange_err.take().is_some() {
                        eprintln!("motion-daemon: exchange recovered");
                    }
                }
                Err(e) => {
                    if exchange_err != Some(e) {
                        eprintln!("motion-daemon: exchange: {e}");
                        exchange_err = Some(e);
                    }
                    continue; // keep pacing; stale ins, no new outs
                }
            }

            // 2. latch the HMI command (PRG_ShmPublisher semantics)
            let fresh = match cmd.poll() {
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

        eprintln!("motion-daemon: shutting down");
        bus.stop().await?;
        Ok(())
    })
}
