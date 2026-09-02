//! Interactive **PDO / process-data console** over EtherCAT — read a subdevice's
//! cyclic *input* image and drive its *output* image, live, through the Rust
//! master. This is the tool for devices that have **no CoE mailbox** (so `sdo`
//! can't touch them): a plain IO block like the MKX 1313 only exposes process
//! data, which exists solely in SAFEOP/OP and is exchanged every cycle.
//!
//! Reaches OP with each subdevice's **default (EEPROM) PDO mapping** — it does
//! *not* write any 402 configuration, so it makes no assumption about drives.
//! A background cyclic loop keeps the bus in OP (feeds the SM watchdog) and
//! refreshes the process image; commands typed on stdin read/patch that image.
//!
//! **Safety:** writing an output byte *physically drives that terminal*, so
//! writes are **allow-listed**: nothing is writable unless named in
//! `--writable <node>[,<node>...]`, and every other node's outputs are driven
//! as all-zero every cycle. On top of that, any subdevice whose vendor id is
//! Control Techniques / Nidec (0x000000F9) is a **drive and stays hard-locked
//! even if listed** (controlword 0 ⇒ stays disabled, no motion); `wo`/`sb`/`cb`
//! on a locked node are refused with the reason. The effective writable set
//! is printed at start-up. On quit — `q`, EOF, **Ctrl-C or SIGTERM** — every
//! output is zeroed for a few cycles, then the group is degraded OP→SAFEOP→
//! PREOP (best effort) so nodes leave OP cleanly instead of via watchdog.
//!
//! Every cycle checks the LRW **working counter** against the value learned
//! on the first all-OP cycle: a node physically dropping off the tail of the
//! chain is invisible to the AL-status check (its FPRD just returns zeros)
//! but shows up as a WKC mismatch. `stats` reports expected/last/errors.
//!
//! Uses `ethercrab` directly (process data is below the `fieldbus-api`
//! normalized-axis seam), same as `sdo`/`bringup`.
//!
//! Usage (needs root / CAP_NET_RAW; on the RT box launch it pinned, e.g.
//! `sudo chrt -f 80 taskset -c 3 pdo ...` — the tool only does `mlockall`):
//!   pdo --ifname enp2s0 [--cycle-us 2000] [--writable <node>[,<node>...]]
//!
//! Commands (node = 0-based chain position; numbers take 0x-hex or decimal):
//!   ls                          list nodes with input/output sizes + writable flag
//!   ri <node>                   show a node's current INPUT bytes (+ set-bit list)
//!   ro <node>                   show a node's current OUTPUT bytes (what we drive)
//!   wo <node> <off> <byte...>   write OUTPUT bytes from offset (each byte 0x-hex/dec)
//!   sb <node> <bit>             set   one OUTPUT bit  (bit = byteOffset*8 + bitInByte)
//!   cb <node> <bit>             clear one OUTPUT bit
//!   watch <node>                print a node's inputs whenever they change
//!   watch off                   stop all watches
//!   stats [on|off]              exchange(scan) + period timing, overruns, WKC; 'on' auto-prints ~1s
//!   help / q

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("pdo: EtherCAT raw sockets are linux-only; run on the target / WSL");
    std::process::exit(1);
}

#[cfg(target_os = "linux")]
fn main() {
    linux::main()
}

#[cfg(target_os = "linux")]
mod linux {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use ethercrab::{
        std::{ethercat_now, tx_rx_task},
        subdevice_group::{NoDc, Op},
        MainDevice, MainDeviceConfig, PduStorage, SubDeviceGroup, SubDeviceState, Timeouts,
    };

    const MAX_FRAMES: usize = 16;
    const MAX_PDU_DATA: usize = PduStorage::element_size(1100);
    const MAX_SUBDEVICES: usize = 16;
    const MAX_PDI: usize = 128;
    type Storage = PduStorage<MAX_FRAMES, MAX_PDU_DATA>;
    /// The group once `request_into_op` has been issued (no DC in this tool).
    type OpGroup = SubDeviceGroup<MAX_SUBDEVICES, MAX_PDI, Op, NoDc>;

    /// Control Techniques / Nidec EtherCAT vendor id — nodes matching this are
    /// treated as drives: outputs forced to zero, writes refused even when
    /// listed in `--writable`.
    const VENDOR_CONTROL_TECHNIQUES: u32 = 0x0000_00f9;

    /// Set by SIGINT/SIGTERM once the cyclic loop is live, so the loop exits
    /// and the zero-outputs epilogue always runs (the `watch` prompt tells the
    /// operator to press Ctrl-C — that must not leave a relay energised).
    static STOP: AtomicBool = AtomicBool::new(false);
    extern "C" fn on_signal(_sig: libc::c_int) {
        STOP.store(true, Ordering::SeqCst);
    }
    fn arm_signals() {
        // Safety: on_signal is async-signal-safe (one atomic store).
        unsafe {
            let h = on_signal as extern "C" fn(libc::c_int) as *const () as libc::sighandler_t;
            libc::signal(libc::SIGINT, h);
            libc::signal(libc::SIGTERM, h);
        }
    }
    fn disarm_signals() {
        unsafe {
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            libc::signal(libc::SIGTERM, libc::SIG_DFL);
        }
    }

    /// Why (or whether) a node's outputs may be written from the console.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Lock {
        /// Listed in `--writable` and not a drive.
        Writable,
        /// Not named in `--writable` — outputs driven as zero.
        NotListed,
        /// CT/Nidec drive (vendor 0xF9) — hard-locked regardless of the list.
        Drive,
    }

    struct Node {
        name: String,
        vendor: u32,
        in_len: usize,
        out_len: usize,
        lock: Lock,
    }

    struct Args {
        ifname: String,
        cycle: Duration,
        /// Nodes (0-based) whose outputs `wo`/`sb`/`cb` may touch.
        writable: Vec<usize>,
    }

    fn parse_args() -> Result<Args, String> {
        let mut a = Args {
            ifname: String::new(),
            cycle: Duration::from_micros(2000),
            writable: Vec::new(),
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
                    let us: u64 = val(i)?.parse().map_err(|_| "bad --cycle-us".to_string())?;
                    if us == 0 {
                        return Err("--cycle-us must be > 0 (0 would spin the bus flat out)".into());
                    }
                    a.cycle = Duration::from_micros(us);
                    i += 1;
                }
                "--writable" => {
                    for tok in val(i)?.split(',') {
                        let tok = tok.trim();
                        if tok.is_empty() {
                            continue;
                        }
                        a.writable.push(parse_u32(tok).map_err(|e| format!("--writable: {e}"))? as usize);
                    }
                    i += 1;
                }
                other => return Err(format!("unknown flag {other}")),
            }
            i += 1;
        }
        if a.ifname.is_empty() {
            return Err("--ifname is required (e.g. --ifname enp2s0)".into());
        }
        a.writable.sort_unstable();
        a.writable.dedup();
        Ok(a)
    }

    fn parse_u32(s: &str) -> Result<u32, String> {
        let s = s.trim();
        let r = if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
            u32::from_str_radix(hex, 16)
        } else {
            s.parse::<u32>()
        };
        r.map_err(|_| format!("bad number: {s}"))
    }

    /// Render a byte image as hex plus the list of set bit indices
    /// (bit = byteOffset*8 + bitInByte, LSB first) — the useful view for
    /// digital I/O.
    fn fmt_image(bytes: &[u8]) -> String {
        if bytes.is_empty() {
            return "(none)".into();
        }
        let hex: Vec<String> = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let mut set = Vec::new();
        for (byte_i, &b) in bytes.iter().enumerate() {
            for bit in 0..8 {
                if b & (1 << bit) != 0 {
                    set.push(byte_i * 8 + bit);
                }
            }
        }
        format!("[{}] {}  set-bits: {:?}", bytes.len(), hex.join(" "), set)
    }

    /// One ring's summary, computed on the cyclic thread (no allocation) and
    /// shipped to the printer thread.
    #[derive(Clone, Copy)]
    struct Summary {
        last: f64,
        n: usize,
        min: f64,
        mean: f64,
        p99: f64,
        max: f64,
    }

    /// Bounded ring of latency samples (µs) for on-demand min/mean/p99/max.
    /// `scratch` is allocated once so `summary` never allocates on the
    /// cyclic thread (it still sorts up to `cap` values — bounded work).
    struct Ring {
        buf: Vec<f64>,
        scratch: Vec<f64>,
        cap: usize,
        next: usize,
        len: usize,
        last: f64,
    }

    impl Ring {
        fn new(cap: usize) -> Ring {
            Ring {
                buf: vec![0.0; cap],
                scratch: vec![0.0; cap],
                cap,
                next: 0,
                len: 0,
                last: 0.0,
            }
        }
        fn push(&mut self, us: f64) {
            self.last = us;
            self.buf[self.next] = us;
            self.next = (self.next + 1) % self.cap;
            if self.len < self.cap {
                self.len += 1;
            }
        }
        fn summary(&mut self) -> Option<Summary> {
            if self.len == 0 {
                return None;
            }
            let n = self.len;
            let v = &mut self.scratch[..n];
            v.copy_from_slice(&self.buf[..n]);
            v.sort_unstable_by(|a, b| a.total_cmp(b));
            let mean = v.iter().sum::<f64>() / n as f64;
            let p99 = v[((n - 1) as f64 * 0.99) as usize];
            Some(Summary {
                last: self.last,
                n,
                min: v[0],
                mean,
                p99,
                max: v[n - 1],
            })
        }
    }

    /// Everything `stats` prints, captured on the cyclic thread as plain
    /// numbers; formatting and the (blocking) write happen on the printer
    /// thread.
    struct StatsSnapshot {
        cycles: usize,
        target_us: f64,
        xchg: Option<Summary>,
        period: Option<Summary>,
        overruns: usize,
        xchg_errors: usize,
        not_op_cycles: usize,
        wkc_expected: Option<u16>,
        wkc_last: u16,
        wkc_errors: usize,
    }

    /// Cyclic thread → printer thread messages. The cyclic loop never touches
    /// stdout itself: a slow terminal or a full pipe must not stall a cycle.
    enum Out {
        Line(String),
        Stats(StatsSnapshot),
    }

    fn fmt_stats(s: &StatsSnapshot, rt: &str) -> String {
        let fx = |r: &Option<Summary>| match r {
            Some(s) => format!(
                "min={:7.1} mean={:7.1} p99={:7.1} max={:7.1}µs  (n={})",
                s.min, s.mean, s.p99, s.max, s.n
            ),
            None => "no samples yet".to_string(),
        };
        let wkc = match s.wkc_expected {
            Some(exp) => format!("expected={exp} last={} errors={}", s.wkc_last, s.wkc_errors),
            None => "not learned yet (waiting for the first all-OP cycle)".to_string(),
        };
        format!(
            "stats @ {} cycles, target {:.0}µs:\n  \
             exchange(scan): last={:.1}µs  {}\n  \
             cycle period:   {}\n  \
             overruns={} (period realigned)  exchange errors={}  cycles with a node not in OP={}\n  \
             wkc: {}\n  \
             rt: {}",
            s.cycles,
            s.target_us,
            s.xchg.map(|x| x.last).unwrap_or(0.0),
            fx(&s.xchg),
            fx(&s.period),
            s.overruns,
            s.xchg_errors,
            s.not_op_cycles,
            wkc,
            rt,
        )
    }

    /// Best-effort `mlockall` (page faults inside the cycle are the classic
    /// jitter source) and a report of the scheduling class we were launched
    /// with. Priority is *not* raised here — the tool is meant to be started
    /// via `chrt -f 80 taskset -c N`, and that is what the returned line
    /// tells the operator to do if it was not.
    fn try_rt_setup() -> String {
        // Safety: plain libc calls on the current process/thread.
        unsafe {
            let mlock = libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE) == 0;
            let mut param: libc::sched_param = std::mem::zeroed();
            libc::sched_getparam(0, &mut param);
            let sched = match libc::sched_getscheduler(0) {
                libc::SCHED_FIFO => format!("SCHED_FIFO prio {}", param.sched_priority),
                libc::SCHED_RR => format!("SCHED_RR prio {}", param.sched_priority),
                _ => "SCHED_OTHER — launch as `sudo chrt -f 80 taskset -c 3 pdo ...` for tight timing"
                    .to_string(),
            };
            format!(
                "mlockall={}  scheduler={sched}",
                if mlock { "ok" } else { "FAILED (needs root/CAP_IPC_LOCK)" }
            )
        }
    }

    pub fn main() {
        let args = match parse_args() {
            Ok(a) => a,
            Err(e) => {
                eprintln!("pdo: {e}");
                std::process::exit(2);
            }
        };
        std::process::exit(run(args));
    }

    fn run(args: Args) -> i32 {
        let storage: &'static Storage = Box::leak(Box::new(Storage::new()));
        let (tx, rx, pdu_loop) = match storage.try_split() {
            Ok(p) => p,
            Err(_) => {
                eprintln!("pdo: PduStorage already split");
                return 1;
            }
        };
        // PDU timeout = a few cycles, not 100 ms: a single lost/late frame must
        // not park the loop for ~50 cycles — that is the order of the nodes'
        // SM watchdog (100 ms default), i.e. one dropped frame would cascade
        // into every node falling to SAFEOP. Floor at 2 ms so a 500 µs cycle
        // still tolerates a normal scheduling hiccup.
        let pdu_timeout = (args.cycle * 4).max(Duration::from_millis(2));
        let maindevice = MainDevice::new(
            pdu_loop,
            Timeouts {
                state_transition: Duration::from_secs(5),
                pdu: pdu_timeout,
                mailbox_response: Duration::from_secs(1),
                ..Timeouts::default()
            },
            MainDeviceConfig::default(),
        );

        let txrx = match tx_rx_task(&args.ifname, tx, rx) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("pdo: open {}: {e}", args.ifname);
                return 1;
            }
        };

        let rt = try_rt_setup();
        eprintln!("pdo: rt: {rt}");
        eprintln!("pdo: pdu timeout {pdu_timeout:?} (4 cycles, min 2 ms)");

        // Printer thread: owns stdout. The cyclic thread only sends messages.
        let (out_tx, out_rx) = mpsc::channel::<Out>();
        let printer = std::thread::spawn(move || {
            for msg in out_rx {
                match msg {
                    Out::Line(s) => println!("{s}"),
                    Out::Stats(s) => println!("{}", fmt_stats(&s, &rt)),
                }
            }
        });

        // Single-threaded I/O: drive the tx/rx task and the cyclic loop on ONE
        // LocalExecutor so the cyclic task polls tx/rx inline on the same thread
        // — no cross-thread wakeup (that handoff was the ~245µs saturation floor
        // of the previous spawn-a-thread design). One thread pinned to one
        // isolated core is also the cleanest RT story.
        let ex = smol::LocalExecutor::new();
        ex.spawn(async move {
            if let Err(e) = txrx.await {
                eprintln!("pdo: tx/rx task exited: {e}");
            }
        })
        .detach();

        let code = smol::block_on(ex.run(drive(&maindevice, &args, out_tx)));
        // `drive` dropped its sender; wait for queued output before exiting.
        let _ = printer.join();
        code
    }

    async fn drive(maindevice: &MainDevice<'static>, args: &Args, out_tx: mpsc::Sender<Out>) -> i32 {
        let out = |s: String| {
            let _ = out_tx.send(Out::Line(s));
        };
        // Scan (PREOP), then bring the whole group to OP on default PDO — no
        // 402 config written, mirroring `bringup --axes 0 --no-dc`.
        let group = match maindevice
            .init_single_group::<MAX_SUBDEVICES, MAX_PDI>(ethercat_now)
            .await
        {
            Ok(g) => g,
            Err(e) => {
                eprintln!("pdo: init/scan: {e}");
                return 1;
            }
        };
        eprintln!("pdo: {} subdevice(s) on {}", group.len(), args.ifname);

        let group = match group.into_pre_op_pdi(maindevice).await {
            Ok(g) => g,
            Err(e) => {
                eprintln!("pdo: configure PDI: {e}");
                return 1;
            }
        };
        let group: OpGroup = match group.request_into_op(maindevice).await {
            Ok(g) => g,
            Err(e) => {
                eprintln!("pdo: request OP: {e}");
                return 1;
            }
        };

        // From here on the nodes are heading for OP with us as their master:
        // every exit path must go through `release` (zero outputs, degrade).
        arm_signals();

        // Drive the cycle until every subdevice reports OP (bounded).
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if STOP.load(Ordering::SeqCst) {
                eprintln!("pdo: interrupted while entering OP");
                release(group, maindevice, args.cycle, true).await;
                return 1;
            }
            match group.tx_rx(maindevice).await {
                Ok(r) => {
                    if r.is_in_state(SubDeviceState::Op) {
                        break;
                    }
                }
                Err(e) => {
                    eprintln!("pdo: tx/rx while entering OP: {e}");
                    release(group, maindevice, args.cycle, false).await;
                    return 1;
                }
            }
            if Instant::now() > deadline {
                eprintln!("pdo: timeout waiting for OP");
                release(group, maindevice, args.cycle, true).await;
                return 1;
            }
            smol::Timer::after(args.cycle).await;
        }
        eprintln!("pdo: OP reached, cyclic exchange live ({:?} cycle)\n", args.cycle);

        // Snapshot per-node identity + IO sizes, decide writability.
        let n = group.len();
        if let Some(&bad) = args.writable.iter().find(|&&w| w >= n) {
            eprintln!("pdo: --writable: no such node {bad} (have 0..{})", n.saturating_sub(1));
            release(group, maindevice, args.cycle, true).await;
            return 2;
        }
        let mut nodes: Vec<Node> = Vec::with_capacity(n);
        for node in 0..n {
            let sd = match group.subdevice(maindevice, node) {
                Ok(sd) => sd,
                Err(e) => {
                    eprintln!("pdo: node {node}: {e}");
                    release(group, maindevice, args.cycle, true).await;
                    return 1;
                }
            };
            let io = sd.io_raw();
            let vendor = sd.identity().vendor_id;
            let listed = args.writable.contains(&node);
            let lock = if vendor == VENDOR_CONTROL_TECHNIQUES {
                if listed {
                    eprintln!(
                        "pdo: --writable {node}: {} is a CT/Nidec drive (vendor 0xF9) — hard-locked, ignoring",
                        sd.name()
                    );
                }
                Lock::Drive
            } else if listed {
                Lock::Writable
            } else {
                Lock::NotListed
            };
            nodes.push(Node {
                name: sd.name().to_string(),
                vendor,
                in_len: io.inputs().len(),
                out_len: io.outputs().len(),
                lock,
            });
        }

        let mut out_desired: Vec<Vec<u8>> = nodes.iter().map(|nd| vec![0u8; nd.out_len]).collect();
        let mut in_snap: Vec<Vec<u8>> = nodes.iter().map(|nd| vec![0u8; nd.in_len]).collect();
        let mut watch: Vec<usize> = Vec::new();
        let mut last_watch: Vec<Vec<u8>> = nodes.iter().map(|nd| vec![0u8; nd.in_len]).collect();

        print_nodes(&nodes);
        let writable: Vec<String> = nodes
            .iter()
            .enumerate()
            .filter(|(_, nd)| nd.lock == Lock::Writable)
            .map(|(i, _)| i.to_string())
            .collect();
        if writable.is_empty() {
            eprintln!("writable nodes: (none) — pass --writable <node>[,<node>] to allow wo/sb/cb");
        } else {
            eprintln!("writable nodes: {}", writable.join(","));
        }
        eprintln!(
            "\ncommands: ls | ri <n> | ro <n> | wo <n> <off> <byte...> | sb/cb <n> <bit> | watch <n> | stats [on|off] | q"
        );
        eprintln!("writing an output drives a REAL terminal. Ctrl-C / q / EOF zero every output before releasing.\n");

        // stdin reader on its own thread → non-blocking drain in the cycle loop.
        let (cmd_tx, cmd_rx) = mpsc::channel::<String>();
        std::thread::spawn(move || {
            let stdin = std::io::stdin();
            let mut line = String::new();
            loop {
                line.clear();
                match stdin.read_line(&mut line) {
                    Ok(0) => {
                        let _ = cmd_tx.send("q".into());
                        break;
                    }
                    Ok(_) => {
                        if cmd_tx.send(line.trim().to_string()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        // ── cyclic loop ─────────────────────────────────────────────────────
        let mut next = Instant::now() + args.cycle;
        let mut running = true;
        let mut xchg = Ring::new(4096);
        let mut period = Ring::new(4096);
        let mut last_wake: Option<Instant> = None;
        let mut cycles: usize = 0;
        let mut overruns: usize = 0;
        let mut auto_stats = false;
        let mut last_stats = Instant::now();
        let target_us = args.cycle.as_secs_f64() * 1e6;
        // Health tracking — each has an "edge" flag so a persistent condition
        // prints once, not 500×/s.
        let mut bus_ok = true;
        let mut xchg_errors: usize = 0;
        let mut xchg_err_run: usize = 0;
        let mut not_op_cycles: usize = 0;
        let mut op_warned = false;
        let mut wkc_expected: Option<u16> = None;
        let mut wkc_last: u16 = 0;
        let mut wkc_errors: usize = 0;
        let mut wkc_warned = false;
        while running {
            smol::Timer::at(next).await;
            let woke = Instant::now();
            next += args.cycle;
            // Overrun: if we are more than a full cycle behind (a PDU timeout,
            // a preemption), realign instead of letting `Timer::at` fire
            // back-to-back — that would burst frames and fake the period stats.
            if woke > next + args.cycle {
                overruns += 1;
                next = woke + args.cycle;
            }

            if let Some(prev) = last_wake {
                period.push((woke - prev).as_secs_f64() * 1e6);
            }
            last_wake = Some(woke);
            cycles += 1;

            if STOP.load(Ordering::SeqCst) {
                eprintln!("pdo: signal received — stopping");
                break;
            }

            // 1. drive outputs (desired image; locked nodes stay all-zero forever)
            for (node, desired) in out_desired.iter().enumerate() {
                if let Ok(sd) = group.subdevice(maindevice, node) {
                    let mut io = sd.io_raw_mut();
                    let out = io.outputs();
                    if !out.is_empty() {
                        out.copy_from_slice(desired);
                    }
                }
            }

            // 2. exchange — time the round-trip (the "single scan" duration)
            let t_ex = Instant::now();
            match group.tx_rx(maindevice).await {
                Ok(r) => {
                    xchg.push(t_ex.elapsed().as_secs_f64() * 1e6);
                    if !bus_ok {
                        bus_ok = true;
                        out(format!("pdo: exchange recovered after {xchg_err_run} error(s)"));
                        xchg_err_run = 0;
                    }

                    // AL state of every node (sees a node dropping to SAFEOP/
                    // PREOP, but NOT a node physically gone — see WKC below).
                    let all_op = r.is_in_state(SubDeviceState::Op);
                    if !all_op {
                        not_op_cycles += 1;
                        if !op_warned {
                            out("pdo: WARNING a subdevice left OP (check the chain)".into());
                            op_warned = true;
                        }
                    } else if op_warned {
                        op_warned = false;
                        out("pdo: all subdevices back in OP".into());
                    }

                    // Working counter: learned on the first all-OP cycle; a
                    // mismatch afterwards means a node did not process the LRW
                    // (dropped off the chain, or its SM is not running).
                    wkc_last = r.working_counter;
                    match wkc_expected {
                        None => {
                            if all_op {
                                wkc_expected = Some(r.working_counter);
                                out(format!("pdo: working counter learned: {}", r.working_counter));
                            }
                        }
                        Some(exp) => {
                            if r.working_counter != exp {
                                wkc_errors += 1;
                                if !wkc_warned {
                                    out(format!(
                                        "pdo: WARNING working counter {} != expected {exp} — a node dropped off the chain? (inputs may be stale)",
                                        r.working_counter
                                    ));
                                    wkc_warned = true;
                                }
                            } else if wkc_warned {
                                wkc_warned = false;
                                out(format!("pdo: working counter back to {exp}"));
                            }
                        }
                    }

                    // 3. read fresh inputs
                    for (node, snap) in in_snap.iter_mut().enumerate() {
                        if let Ok(sd) = group.subdevice(maindevice, node) {
                            let io = sd.io_raw();
                            let inp = io.inputs();
                            if !inp.is_empty() {
                                snap.copy_from_slice(inp);
                            }
                        }
                    }

                    // 4. watch: print any watched node whose inputs changed
                    for &node in &watch {
                        if in_snap[node] != last_watch[node] {
                            out(format!("  [watch] node {node} in {}", fmt_image(&in_snap[node])));
                            last_watch[node].copy_from_slice(&in_snap[node]);
                        }
                    }
                }
                Err(e) => {
                    // Not sampled into `xchg`: a timeout is not a scan time.
                    xchg_errors += 1;
                    xchg_err_run += 1;
                    if bus_ok {
                        bus_ok = false;
                        out(format!("pdo: exchange error: {e} (bus down? still draining commands — 'q' works)"));
                    }
                }
            }

            // 5. handle typed commands — ALWAYS, even with the bus down, so
            //    'q' is honoured when the cable is pulled.
            while let Ok(cmd) = cmd_rx.try_recv() {
                if cmd.is_empty() || cmd.starts_with('#') {
                    continue;
                }
                let tok: Vec<&str> = cmd.split_whitespace().collect();
                // `stats` lives here (needs the timing rings this loop owns).
                if tok[0] == "stats" {
                    match tok.get(1).copied() {
                        Some("on") => {
                            auto_stats = true;
                            out("stats: auto-print on (~1s) — 'stats off' to stop".into());
                        }
                        Some("off") => {
                            auto_stats = false;
                            out("stats: auto-print off".into());
                        }
                        _ => {
                            let _ = out_tx.send(Out::Stats(StatsSnapshot {
                                cycles,
                                target_us,
                                xchg: xchg.summary(),
                                period: period.summary(),
                                overruns,
                                xchg_errors,
                                not_op_cycles,
                                wkc_expected,
                                wkc_last,
                                wkc_errors,
                            }));
                        }
                    }
                    continue;
                }
                match handle(&tok, &nodes, &in_snap, &mut out_desired, &mut watch, &mut last_watch) {
                    Ok(Some(msg)) => out(msg),
                    Ok(None) => {}
                    Err(Cmd::Quit) => running = false,
                    Err(Cmd::Msg(e)) => out(format!("pdo: {e}")),
                }
            }

            // 6. periodic stats auto-print
            if auto_stats && last_stats.elapsed() >= Duration::from_secs(1) {
                let _ = out_tx.send(Out::Stats(StatsSnapshot {
                    cycles,
                    target_us,
                    xchg: xchg.summary(),
                    period: period.summary(),
                    overruns,
                    xchg_errors,
                    not_op_cycles,
                    wkc_expected,
                    wkc_last,
                    wkc_errors,
                }));
                last_stats = Instant::now();
            }
        }

        // ── safe stop: zero every output, flush a few cycles, then release ──
        eprintln!("pdo: zeroing outputs and releasing bus…");
        release(group, maindevice, args.cycle, bus_ok).await;
        0
    }

    /// The one exit path once we own an OP-bound group: drive all-zero
    /// outputs for a few cycles (the physical safe state), then — if the bus
    /// still answers — walk the group OP→SAFEOP→PREOP so nodes leave OP by
    /// request instead of by SM-watchdog timeout (AL status 0x001B) when our
    /// frames stop.
    async fn release(group: OpGroup, maindevice: &MainDevice<'static>, cycle: Duration, degrade: bool) {
        for _ in 0..5 {
            for node in 0..group.len() {
                if let Ok(sd) = group.subdevice(maindevice, node) {
                    sd.io_raw_mut().outputs().fill(0);
                }
            }
            let _ = group.tx_rx(maindevice).await;
            smol::Timer::after(cycle).await;
        }
        // Outputs are zero on the wire now. The state degrade is nice-to-have
        // and may sit in state_transition timeouts on a broken chain, so let
        // a second Ctrl-C terminate the process from here on.
        disarm_signals();
        if !degrade {
            eprintln!("pdo: bus not responding — skipping OP→SAFEOP→PREOP degrade");
            return;
        }
        match group.into_safe_op(maindevice).await {
            Ok(g) => match g.into_pre_op(maindevice).await {
                Ok(_) => eprintln!("pdo: nodes degraded to PREOP"),
                Err(e) => eprintln!("pdo: SAFEOP→PREOP: {e} (ignored)"),
            },
            Err(e) => eprintln!("pdo: OP→SAFEOP: {e} (ignored)"),
        }
    }

    fn lock_label(lock: Lock) -> &'static str {
        match lock {
            Lock::Writable => "writable",
            Lock::NotListed => "locked (not in --writable, outputs forced 0)",
            Lock::Drive => "LOCKED (CT drive, outputs forced 0)",
        }
    }

    fn print_nodes(nodes: &[Node]) {
        eprintln!("nodes (process-data sizes):");
        for (i, nd) in nodes.iter().enumerate() {
            eprintln!(
                "  node {i}: {:<16} vendor={:#010x}  in={}B out={}B  [{}]",
                nd.name,
                nd.vendor,
                nd.in_len,
                nd.out_len,
                lock_label(nd.lock),
            );
        }
    }

    /// Command result: either a line to print (Some) / nothing (None), or a
    /// control signal (quit / error message).
    enum Cmd {
        Quit,
        Msg(String),
    }

    fn handle(
        tok: &[&str],
        nodes: &[Node],
        in_snap: &[Vec<u8>],
        out_desired: &mut [Vec<u8>],
        watch: &mut Vec<usize>,
        last_watch: &mut [Vec<u8>],
    ) -> Result<Option<String>, Cmd> {
        let node_arg = |idx: usize| -> Result<usize, Cmd> {
            let node = parse_u32(tok.get(idx).copied().ok_or_else(|| Cmd::Msg("missing node".into()))?)
                .map_err(Cmd::Msg)? as usize;
            if node >= nodes.len() {
                return Err(Cmd::Msg(format!("no such node {node} (have 0..{})", nodes.len() - 1)));
            }
            Ok(node)
        };

        match tok[0] {
            "q" | "quit" | "exit" => Err(Cmd::Quit),
            "help" | "?" | "h" => Ok(Some(HELP.into())),
            "ls" | "list" => {
                let mut s = String::from("nodes:");
                for (i, nd) in nodes.iter().enumerate() {
                    s.push_str(&format!(
                        "\n  node {i}: {} in={}B out={}B [{}]",
                        nd.name,
                        nd.in_len,
                        nd.out_len,
                        lock_label(nd.lock)
                    ));
                }
                Ok(Some(s))
            }
            "ri" => {
                let node = node_arg(1)?;
                Ok(Some(format!("node {node} in  {}", fmt_image(&in_snap[node]))))
            }
            "ro" => {
                let node = node_arg(1)?;
                Ok(Some(format!("node {node} out {}", fmt_image(&out_desired[node]))))
            }
            "wo" => {
                let node = node_arg(1)?;
                guard_writable(node, nodes)?;
                let off = parse_u32(tok.get(2).copied().ok_or_else(|| Cmd::Msg("wo <node> <off> <byte...>".into()))?)
                    .map_err(Cmd::Msg)? as usize;
                let bytes: Vec<u8> = tok[3..]
                    .iter()
                    .map(|t| parse_u32(t).and_then(|v| u8::try_from(v).map_err(|_| format!("byte out of range: {t}"))))
                    .collect::<Result<_, _>>()
                    .map_err(Cmd::Msg)?;
                if bytes.is_empty() {
                    return Err(Cmd::Msg("wo: no bytes given".into()));
                }
                if off + bytes.len() > out_desired[node].len() {
                    return Err(Cmd::Msg(format!(
                        "wo: off {off} + {} byte(s) exceeds node {node} output size {}B",
                        bytes.len(),
                        out_desired[node].len()
                    )));
                }
                out_desired[node][off..off + bytes.len()].copy_from_slice(&bytes);
                Ok(Some(format!("node {node} out {}", fmt_image(&out_desired[node]))))
            }
            "sb" | "cb" => {
                let node = node_arg(1)?;
                guard_writable(node, nodes)?;
                let bit = parse_u32(tok.get(2).copied().ok_or_else(|| Cmd::Msg(format!("{} <node> <bit>", tok[0])))?)
                    .map_err(Cmd::Msg)? as usize;
                let byte = bit / 8;
                if byte >= out_desired[node].len() {
                    return Err(Cmd::Msg(format!(
                        "bit {bit} (byte {byte}) exceeds node {node} output size {}B",
                        out_desired[node].len()
                    )));
                }
                let mask = 1u8 << (bit % 8);
                if tok[0] == "sb" {
                    out_desired[node][byte] |= mask;
                } else {
                    out_desired[node][byte] &= !mask;
                }
                Ok(Some(format!("node {node} out {}", fmt_image(&out_desired[node]))))
            }
            "watch" => {
                match tok.get(1).copied() {
                    Some("off") | Some("stop") => {
                        watch.clear();
                        Ok(Some("watch: cleared".into()))
                    }
                    Some(_) => {
                        let node = node_arg(1)?;
                        if !watch.contains(&node) {
                            watch.push(node);
                            // Force a print next cycle by desyncing last_watch.
                            for b in last_watch[node].iter_mut() {
                                *b = !*b;
                            }
                        }
                        Ok(Some(format!("watch: node {node} (Ctrl-C or 'watch off' to stop)")))
                    }
                    None => Err(Cmd::Msg("watch <node> | watch off".into())),
                }
            }
            other => Err(Cmd::Msg(format!("unknown command '{other}' (try 'help')"))),
        }
    }

    fn guard_writable(node: usize, nodes: &[Node]) -> Result<(), Cmd> {
        let nd = &nodes[node];
        match nd.lock {
            Lock::Drive => {
                return Err(Cmd::Msg(format!(
                    "node {node} ({}) is a CT/Nidec drive (vendor 0xF9) — outputs hard-locked to 0, refusing write",
                    nd.name
                )));
            }
            Lock::NotListed => {
                return Err(Cmd::Msg(format!(
                    "node {node} ({}) is not in --writable — outputs locked to 0, refusing write (restart with --writable {node})",
                    nd.name
                )));
            }
            Lock::Writable => {}
        }
        if nd.out_len == 0 {
            return Err(Cmd::Msg(format!("node {node} has no outputs")));
        }
        Ok(())
    }

    const HELP: &str = "commands (node = 0-based; numbers 0x-hex or decimal):\n  \
        ls                          nodes + IO sizes + writable flag\n  \
        ri <node>                   show INPUT bytes (+ set-bit list)\n  \
        ro <node>                   show OUTPUT bytes we drive\n  \
        wo <node> <off> <byte...>   write OUTPUT bytes from offset (node must be in --writable)\n  \
        sb <node> <bit>             set   one output bit (bit = off*8 + bitInByte)\n  \
        cb <node> <bit>             clear one output bit\n  \
        watch <node> | watch off    print inputs on change / stop\n  \
        stats [on|off]              exchange(scan) + period timing, overruns, WKC; 'on' auto-prints ~1s\n  \
        q                           quit (zeroes outputs first; Ctrl-C / SIGTERM do the same)";
}
