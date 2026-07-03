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
//! **Safety:** writing an output byte *physically drives that terminal*. To
//! keep this safe next to a live servo, any subdevice whose vendor id is
//! Control Techniques / Nidec (0x000000F9) is treated as a **drive and its
//! outputs are forced to zero every cycle** (controlword 0 ⇒ stays disabled,
//! no motion); `wo`/`sb`/`cb` are refused on it. Only mailbox-less IO (the
//! MKX) is writable. On quit, all outputs are zeroed before releasing the bus.
//!
//! Uses `ethercrab` directly (process data is below the `fieldbus-api`
//! normalized-axis seam), same as `sdo`/`bringup`.
//!
//! Usage (needs root / CAP_NET_RAW):
//!   pdo --ifname enp2s0 [--cycle-us 2000]
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
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use ethercrab::{
        std::{ethercat_now, tx_rx_task},
        MainDevice, MainDeviceConfig, PduStorage, SubDeviceState, Timeouts,
    };

    const MAX_FRAMES: usize = 16;
    const MAX_PDU_DATA: usize = PduStorage::element_size(1100);
    const MAX_SUBDEVICES: usize = 16;
    const MAX_PDI: usize = 128;
    type Storage = PduStorage<MAX_FRAMES, MAX_PDU_DATA>;

    /// Control Techniques / Nidec EtherCAT vendor id — nodes matching this are
    /// treated as drives: outputs forced to zero, writes refused.
    const VENDOR_CONTROL_TECHNIQUES: u32 = 0x0000_00f9;

    struct Node {
        name: String,
        vendor: u32,
        in_len: usize,
        out_len: usize,
        writable: bool,
    }

    struct Args {
        ifname: String,
        cycle: Duration,
    }

    fn parse_args() -> Result<Args, String> {
        let mut a = Args {
            ifname: String::new(),
            cycle: Duration::from_micros(2000),
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
                other => return Err(format!("unknown flag {other}")),
            }
            i += 1;
        }
        if a.ifname.is_empty() {
            return Err("--ifname is required (e.g. --ifname enp2s0)".into());
        }
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

    pub fn main() {
        let args = match parse_args() {
            Ok(a) => a,
            Err(e) => {
                eprintln!("pdo: {e}");
                std::process::exit(2);
            }
        };
        std::process::exit(smol::block_on(run(args)));
    }

    async fn run(args: Args) -> i32 {
        let storage: &'static Storage = Box::leak(Box::new(Storage::new()));
        let (tx, rx, pdu_loop) = match storage.try_split() {
            Ok(p) => p,
            Err(_) => {
                eprintln!("pdo: PduStorage already split");
                return 1;
            }
        };
        let maindevice = MainDevice::new(
            pdu_loop,
            Timeouts {
                state_transition: Duration::from_secs(5),
                pdu: Duration::from_millis(100),
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
        if std::thread::Builder::new()
            .name("ecat-txrx".into())
            .spawn(move || {
                if let Err(e) = smol::block_on(txrx) {
                    eprintln!("pdo: tx/rx task exited: {e}");
                }
            })
            .is_err()
        {
            eprintln!("pdo: spawn tx/rx thread failed");
            return 1;
        }

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

        let group = match group.into_pre_op_pdi(&maindevice).await {
            Ok(g) => g,
            Err(e) => {
                eprintln!("pdo: configure PDI: {e}");
                return 1;
            }
        };
        let group = match group.request_into_op(&maindevice).await {
            Ok(g) => g,
            Err(e) => {
                eprintln!("pdo: request OP: {e}");
                return 1;
            }
        };

        // Drive the cycle until every subdevice reports OP (bounded).
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match group.tx_rx(&maindevice).await {
                Ok(r) => {
                    if r.is_in_state(SubDeviceState::Op) {
                        break;
                    }
                }
                Err(e) => {
                    eprintln!("pdo: tx/rx while entering OP: {e}");
                    return 1;
                }
            }
            if Instant::now() > deadline {
                eprintln!("pdo: timeout waiting for OP");
                return 1;
            }
            smol::Timer::after(args.cycle).await;
        }
        eprintln!("pdo: OP reached, cyclic exchange live ({:?} cycle)\n", args.cycle);

        // Snapshot per-node identity + IO sizes, decide writability.
        let n = group.len();
        let mut nodes: Vec<Node> = Vec::with_capacity(n);
        for node in 0..n {
            let sd = match group.subdevice(&maindevice, node) {
                Ok(sd) => sd,
                Err(e) => {
                    eprintln!("pdo: node {node}: {e}");
                    return 1;
                }
            };
            let io = sd.io_raw();
            let vendor = sd.identity().vendor_id;
            let writable = vendor != VENDOR_CONTROL_TECHNIQUES;
            nodes.push(Node {
                name: sd.name().to_string(),
                vendor,
                in_len: io.inputs().len(),
                out_len: io.outputs().len(),
                writable,
            });
        }

        let mut out_desired: Vec<Vec<u8>> = nodes.iter().map(|nd| vec![0u8; nd.out_len]).collect();
        let mut in_snap: Vec<Vec<u8>> = nodes.iter().map(|nd| vec![0u8; nd.in_len]).collect();
        let mut watch: Vec<usize> = Vec::new();
        let mut last_watch: Vec<Vec<u8>> = nodes.iter().map(|nd| vec![0u8; nd.in_len]).collect();

        print_nodes(&nodes);
        eprintln!(
            "\ncommands: ls | ri <n> | ro <n> | wo <n> <off> <byte...> | sb/cb <n> <bit> | watch <n> | watch off | q"
        );
        eprintln!("writing an output drives a REAL terminal. CT-drive outputs are locked to 0.\n");

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
        let mut offline_warned = false;
        while running {
            smol::Timer::at(next).await;
            next += args.cycle;

            // 1. drive outputs (desired image; CT nodes stay all-zero forever)
            for node in 0..n {
                if let Ok(sd) = group.subdevice(&maindevice, node) {
                    let mut io = sd.io_raw_mut();
                    let out = io.outputs();
                    if !out.is_empty() {
                        out.copy_from_slice(&out_desired[node]);
                    }
                }
            }

            // 2. exchange
            match group.tx_rx(&maindevice).await {
                Ok(r) => {
                    let ok = r.is_in_state(SubDeviceState::Op);
                    if !ok && !offline_warned {
                        println!("pdo: WARNING a subdevice left OP (check the chain)");
                        offline_warned = true;
                    } else if ok {
                        offline_warned = false;
                    }
                }
                Err(e) => {
                    println!("pdo: exchange error: {e}");
                    continue;
                }
            }

            // 3. read fresh inputs
            for node in 0..n {
                if let Ok(sd) = group.subdevice(&maindevice, node) {
                    let io = sd.io_raw();
                    let inp = io.inputs();
                    if !inp.is_empty() {
                        in_snap[node].copy_from_slice(inp);
                    }
                }
            }

            // 4. watch: print any watched node whose inputs changed
            for &node in &watch {
                if in_snap[node] != last_watch[node] {
                    println!(
                        "  [watch] node {node} in {}",
                        fmt_image(&in_snap[node])
                    );
                    last_watch[node].copy_from_slice(&in_snap[node]);
                }
            }

            // 5. handle typed commands
            while let Ok(cmd) = cmd_rx.try_recv() {
                if cmd.is_empty() || cmd.starts_with('#') {
                    continue;
                }
                let tok: Vec<&str> = cmd.split_whitespace().collect();
                match handle(&tok, &nodes, &in_snap, &mut out_desired, &mut watch, &mut last_watch) {
                    Ok(Some(msg)) => println!("{msg}"),
                    Ok(None) => {}
                    Err(Cmd::Quit) => running = false,
                    Err(Cmd::Msg(e)) => println!("pdo: {e}"),
                }
            }
        }

        // ── safe stop: zero every output, flush a few cycles, then release ──
        eprintln!("pdo: zeroing outputs and releasing bus…");
        for buf in &mut out_desired {
            buf.iter_mut().for_each(|b| *b = 0);
        }
        for _ in 0..5 {
            for node in 0..n {
                if let Ok(sd) = group.subdevice(&maindevice, node) {
                    let mut io = sd.io_raw_mut();
                    let out = io.outputs();
                    if !out.is_empty() {
                        out.copy_from_slice(&out_desired[node]);
                    }
                }
            }
            let _ = group.tx_rx(&maindevice).await;
            smol::Timer::after(args.cycle).await;
        }
        0
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
                if nd.writable {
                    "writable"
                } else {
                    "LOCKED (drive, outputs forced 0)"
                },
            );
        }
    }

    /// Command result: either a line to print (Some) / nothing (None), or a
    /// control signal (quit / error message).
    enum Cmd {
        Quit,
        Msg(String),
    }

    #[allow(clippy::too_many_arguments)]
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
                        if nd.writable { "writable" } else { "locked" }
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
        if !nodes[node].writable {
            return Err(Cmd::Msg(format!(
                "node {node} ({}) is a drive — outputs are locked to 0, refusing write",
                nodes[node].name
            )));
        }
        if nodes[node].out_len == 0 {
            return Err(Cmd::Msg(format!("node {node} has no outputs")));
        }
        Ok(())
    }

    const HELP: &str = "commands (node = 0-based; numbers 0x-hex or decimal):\n  \
        ls                          nodes + IO sizes + writable flag\n  \
        ri <node>                   show INPUT bytes (+ set-bit list)\n  \
        ro <node>                   show OUTPUT bytes we drive\n  \
        wo <node> <off> <byte...>   write OUTPUT bytes from offset\n  \
        sb <node> <bit>             set   one output bit (bit = off*8 + bitInByte)\n  \
        cb <node> <bit>             clear one output bit\n  \
        watch <node> | watch off    print inputs on change / stop\n  \
        q                           quit (zeroes outputs first)";
}
