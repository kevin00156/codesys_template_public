//! EtherCAT bring-up: scan the chain, configure drives, reach OP, then run
//! the cyclic exchange for a fixed duration and print timing statistics
//! (wake jitter, exchange duration, WKC health). This is the Phase 3
//! acceptance artifact and the tool to run first on real hardware — the DC
//! jitter numbers it prints are the kickoff §6 "實機驗證清單" data.
//!
//! Runs entirely through the `fieldbus-api` seam — the exact code path the
//! daemon uses.
//!
//! Usage (root or CAP_NET_RAW):
//!   bringup --ifname eth0 [--cycle-us 1000] [--duration-s 10] [--axes 1]
//!           [--first-subdevice 0] [--scale 10000] [--no-dc] [--no-din]
//!           [--no-pdo-config] [--enable]
//!
//! `--enable` additionally walks the CiA 402 power chain to Operation
//! Enabled and holds the current position (no motion). Default off: plain
//! cyclic exchange with drives left disabled.

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("bringup: EtherCAT raw sockets are linux-only; run on the target / WSL");
    std::process::exit(1);
}

#[cfg(target_os = "linux")]
fn main() {
    linux::main()
}

#[cfg(target_os = "linux")]
mod linux {
    use std::time::{Duration, Instant};

    use fieldbus_api::{AxisIn, AxisOut, Fieldbus, Setpoint};
    use fieldbus_ethercat::{AxisMapping, EcatConfig, EthercatBackend};

    struct Args {
        ifname: String,
        cycle: Duration,
        duration: Duration,
        axes: usize,
        first_subdevice: usize,
        scale: f64,
        dc: bool,
        din: bool,
        pdo_config: bool,
        enable: bool,
    }

    fn parse_args() -> Result<Args, String> {
        let mut a = Args {
            ifname: String::new(),
            cycle: Duration::from_micros(1000),
            duration: Duration::from_secs(10),
            axes: 1,
            first_subdevice: 0,
            scale: 10_000.0,
            dc: true,
            din: true,
            pdo_config: true,
            enable: false,
        };
        let argv: Vec<String> = std::env::args().skip(1).collect();
        let mut i = 0;
        while i < argv.len() {
            let val = |i: usize| -> Result<&String, String> {
                argv.get(i + 1).ok_or_else(|| format!("{} needs a value", argv[i]))
            };
            match argv[i].as_str() {
                "--ifname" => {
                    a.ifname = val(i)?.clone();
                    i += 1;
                }
                "--cycle-us" => {
                    a.cycle = Duration::from_micros(
                        val(i)?.parse().map_err(|_| "bad --cycle-us".to_string())?,
                    );
                    i += 1;
                }
                "--duration-s" => {
                    a.duration = Duration::from_secs(
                        val(i)?.parse().map_err(|_| "bad --duration-s".to_string())?,
                    );
                    i += 1;
                }
                "--axes" => {
                    a.axes = val(i)?.parse().map_err(|_| "bad --axes".to_string())?;
                    i += 1;
                }
                "--first-subdevice" => {
                    a.first_subdevice =
                        val(i)?.parse().map_err(|_| "bad --first-subdevice".to_string())?;
                    i += 1;
                }
                "--scale" => {
                    a.scale = val(i)?.parse().map_err(|_| "bad --scale".to_string())?;
                    i += 1;
                }
                "--no-dc" => a.dc = false,
                "--no-din" => a.din = false,
                "--no-pdo-config" => a.pdo_config = false,
                "--enable" => a.enable = true,
                other => return Err(format!("unknown flag {other}")),
            }
            i += 1;
        }
        if a.ifname.is_empty() {
            return Err("--ifname is required (e.g. --ifname eth0)".into());
        }
        Ok(a)
    }

    /// Latency histogram in µs buckets + exact percentiles from stored samples.
    struct Stats {
        name: &'static str,
        samples_us: Vec<f64>,
    }

    const BUCKETS_US: &[f64] = &[10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0];

    impl Stats {
        fn new(name: &'static str, capacity: usize) -> Stats {
            Stats {
                name,
                samples_us: Vec::with_capacity(capacity),
            }
        }

        fn push(&mut self, d: Duration) {
            self.samples_us.push(d.as_secs_f64() * 1e6);
        }

        fn report(&mut self) {
            if self.samples_us.is_empty() {
                println!("{:<18} no samples", self.name);
                return;
            }
            self.samples_us.sort_by(|a, b| a.total_cmp(b));
            let n = self.samples_us.len();
            let pct = |p: f64| self.samples_us[(((n - 1) as f64) * p) as usize];
            let mean: f64 = self.samples_us.iter().sum::<f64>() / n as f64;
            println!(
                "{:<18} n={:<7} min={:8.1}µs mean={:8.1}µs p99={:8.1}µs max={:8.1}µs",
                self.name,
                n,
                self.samples_us[0],
                mean,
                pct(0.99),
                self.samples_us[n - 1],
            );
            let mut lo = 0.0;
            for &hi in BUCKETS_US {
                let c = self
                    .samples_us
                    .iter()
                    .filter(|&&v| v >= lo && v < hi)
                    .count();
                if c > 0 {
                    println!("    [{lo:>6.0}..{hi:>6.0}µs) {c}");
                }
                lo = hi;
            }
            let c = self.samples_us.iter().filter(|&&v| v >= lo).count();
            if c > 0 {
                println!("    [{lo:>6.0}µs..    ) {c}");
            }
        }
    }

    fn try_rt_setup() {
        // Best effort — needs root/CAP_SYS_NICE; the example still runs
        // (with worse jitter) without it.
        unsafe {
            if libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE) != 0 {
                eprintln!("bringup: mlockall failed (running without memory lock)");
            }
            let param = libc::sched_param { sched_priority: 80 };
            if libc::sched_setscheduler(0, libc::SCHED_FIFO, &param) != 0 {
                eprintln!("bringup: SCHED_FIFO failed (running with normal scheduling)");
            }
        }
    }

    pub fn main() {
        let args = match parse_args() {
            Ok(a) => a,
            Err(e) => {
                eprintln!("bringup: {e}");
                std::process::exit(2);
            }
        };

        let mut cfg = EcatConfig::new(&args.ifname, 0);
        cfg.cycle = args.cycle;
        cfg.dc = args.dc;
        cfg.configure_pdo = args.pdo_config;
        cfg.map_digital_inputs = args.din;
        cfg.axes = (0..args.axes)
            .map(|i| AxisMapping {
                subdevice: args.first_subdevice + i,
                scale: args.scale,
            })
            .collect();

        println!(
            "bringup: {} axes on {}, cycle {:?}, dc={}, duration {:?}",
            args.axes, args.ifname, args.cycle, args.dc, args.duration
        );

        try_rt_setup();

        let mut bus = EthercatBackend::new(cfg);
        let code = smol::block_on(run(&mut bus, &args));
        std::process::exit(code);
    }

    async fn run(bus: &mut EthercatBackend, args: &Args) -> i32 {
        let t0 = Instant::now();
        if let Err(e) = bus.start().await {
            eprintln!("bringup: start failed: {e}");
            return 1;
        }
        println!("bringup: OP reached in {:?}", t0.elapsed());

        let cycles = (args.duration.as_secs_f64() / args.cycle.as_secs_f64()) as usize;
        let mut wake_late = Stats::new("wake latency", cycles + 8);
        let mut period_jit = Stats::new("period |Δ-cycle|", cycles + 8);
        let mut xchg_time = Stats::new("exchange time", cycles + 8);

        let mut outs = vec![AxisOut::default(); args.axes];
        let mut ins = vec![AxisIn::default(); args.axes];
        let mut offline_cycles = 0usize;
        let mut xchg_errors = 0usize;

        let mut next = Instant::now() + args.cycle;
        let mut last_wake: Option<Instant> = None;
        for _ in 0..cycles {
            smol::Timer::at(next).await;
            let now = Instant::now();
            wake_late.push(now.saturating_duration_since(next));
            if let Some(prev) = last_wake {
                let period = now - prev;
                let jitter = if period > args.cycle {
                    period - args.cycle
                } else {
                    args.cycle - period
                };
                period_jit.push(jitter);
            }
            last_wake = Some(now);
            next += args.cycle;

            if args.enable {
                for (i, out) in outs.iter_mut().enumerate() {
                    out.enable = true;
                    // Hold position under CSP — enabled drives follow their
                    // own actual position, i.e. zero motion.
                    out.setpoint = Setpoint::CyclicPosition {
                        pos: ins[i].act_pos,
                        vel_ff: 0.0,
                    };
                }
            }

            let tx = Instant::now();
            match bus.exchange(&outs, &mut ins).await {
                Ok(st) => {
                    if !st.all_axes_responding {
                        offline_cycles += 1;
                    }
                }
                Err(_) => xchg_errors += 1,
            }
            xchg_time.push(tx.elapsed());

            while let Some(ev) = bus.poll_event() {
                println!("bringup: event {ev:?}");
            }
        }

        println!("\n=== cyclic timing over {} cycles @ {:?} ===", cycles, args.cycle);
        wake_late.report();
        period_jit.report();
        xchg_time.report();
        println!(
            "offline cycles: {offline_cycles}   exchange errors: {xchg_errors}"
        );
        for (i, axis) in ins.iter().enumerate() {
            println!(
                "axis{i}: drive={:?} pos={:.4} vel={:.4} limits(-{},+{})",
                axis.drive, axis.act_pos, axis.act_vel, axis.neg_limit, axis.pos_limit
            );
        }

        if let Err(e) = bus.stop().await {
            eprintln!("bringup: stop: {e}");
        }
        0
    }
}
