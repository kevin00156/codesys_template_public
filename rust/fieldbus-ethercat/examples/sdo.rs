//! Interactive CoE **SDO console** over EtherCAT — read and write the object
//! dictionary of any subdevice on the chain, live, through the Rust master.
//! This is the tangible "yes, Rust really is talking EtherCAT" proof: read
//! `0x1018` (Identity) and it matches the topology scan; read `0x1008` and the
//! drive hands back its own device-name string.
//!
//! The bus is taken only to **PREOP** — the CoE mailbox is already active
//! there, and PREOP means *no process data and no motion*, so poking
//! parameters is as safe as it gets (nothing is commanded to move).
//!
//! Uses `ethercrab` directly rather than the `fieldbus-api` seam: an SDO is a
//! raw object-dictionary access, inherently bus-specific, and sits *below* the
//! normalized-axis abstraction (that is exactly why `AcyclicAccess` on the
//! backend still returns `Unsupported`, README ADR-12). This example doubles as
//! a working prototype of the acyclic channel that ADR-12 will fold into
//! `EthercatBackend`.
//!
//! Usage (needs root / CAP_NET_RAW):
//!   sdo --ifname enp2s0 [--allow-vendor-write]
//!
//! Then type commands. It reads stdin line by line, so it works both
//! interactively and piped:
//!   printf 'r 0 0x1018 1 u32\nr 0 0x1008 0 str\n' | sudo ./sdo --ifname enp2s0
//!
//! Address syntax: an object is `<idx> <sub>` (two tokens) OR `<idx>:<sub>`
//! (one token, matching CODESYS' `16#2008:16#14` — just prefix both with 0x).
//! idx/sub take 0x-hex or decimal; node = 0-based chain position.
//! Signed values (i8/i16/i32) take decimal with optional `-`, or hex — either
//! `-0x10` or the two's-complement pattern of the target width (`0xffff` as
//! i16 = -1).
//!
//! **Vendor objects (0x2000..=0x5FFF) are write-locked by default.** On the
//! CT drive they are the parameter menus (`0x2000 + menu`, sub = param) and
//! a write takes effect *immediately*, PREOP or not — e.g. Pr 6.15 (drive
//! enable). Pass `--allow-vendor-write` to permit them; profile/comm objects
//! (0x1000.. and 0x6000..) are not affected by the lock.
//!
//! Commands:
//!   ls                                     list the subdevices from the scan
//!   r <node> <idx[:sub]> [sub] [type]      read  SDO  (type: u8 u16 u32 i8 i16 i32 str hex; default hex)
//!   w <node> <idx[:sub]> [sub] <type> <v>  write SDO  (type: u8 u16 u32 i8 i16 i32)
//!   watch <node> <idx[:sub]> <type> [...]  poll a set of objects, print on change (Ctrl-C to stop;
//!                                          a transient SDO failure is logged, 5 in a row abort)
//!   help                                   show this list
//!   q                                      quit
//!
//! Examples (CT SI-EtherCAT drive on node 0):
//!   r 0 0x1008 str                         device name
//!   r 0 0x2008:0x14 u16                    DigitalInputs (CODESYS 16#2008:16#14)
//!   watch 0 0x2008:0x14 hex 0x2013:0x0a u16   watch DigitalInputs + StatusWord2 live

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("sdo: EtherCAT raw sockets are linux-only; run on the target / WSL");
    std::process::exit(1);
}

#[cfg(target_os = "linux")]
fn main() {
    linux::main()
}

#[cfg(target_os = "linux")]
mod linux {
    use std::io::Write;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use ethercrab::{
        std::{ethercat_now, tx_rx_task},
        MainDevice, MainDeviceConfig, PduStorage, SubDeviceGroup, Timeouts,
    };

    // Same sizing as the real backend so behaviour matches (backend.rs).
    const MAX_FRAMES: usize = 16;
    const MAX_PDU_DATA: usize = PduStorage::element_size(1100);
    const MAX_SUBDEVICES: usize = 16;
    const MAX_PDI: usize = 128;
    const WATCH_PERIOD_MS: u64 = 200;
    type Storage = PduStorage<MAX_FRAMES, MAX_PDU_DATA>;
    // `init_single_group` hands back a PREOP group (state param defaults to PreOp).
    type Group = SubDeviceGroup<MAX_SUBDEVICES, MAX_PDI>;

    /// Set by SIGINT while a `watch` is running so the loop can stop and hand
    /// control back to the prompt instead of killing the process. Only armed
    /// during `watch`; the default terminate behaviour is restored afterwards.
    static WATCH_STOP: AtomicBool = AtomicBool::new(false);
    extern "C" fn on_sigint(_sig: libc::c_int) {
        WATCH_STOP.store(true, Ordering::SeqCst);
    }

    /// Manufacturer-specific profile area of the CoE object dictionary. On
    /// the CT drive these are the live parameter menus.
    const VENDOR_OBJECTS: std::ops::RangeInclusive<u16> = 0x2000..=0x5FFF;

    struct Args {
        ifname: String,
        /// `--allow-vendor-write`: permit `w` on 0x2000..=0x5FFF.
        allow_vendor_write: bool,
    }

    fn parse_args() -> Result<Args, String> {
        let argv: Vec<String> = std::env::args().skip(1).collect();
        let mut i = 0;
        let mut a = Args {
            ifname: String::new(),
            allow_vendor_write: false,
        };
        while i < argv.len() {
            match argv[i].as_str() {
                "--ifname" => {
                    a.ifname = argv
                        .get(i + 1)
                        .ok_or_else(|| "--ifname needs a value".to_string())?
                        .clone();
                    i += 1;
                }
                "--allow-vendor-write" => a.allow_vendor_write = true,
                other => return Err(format!("unknown flag {other}")),
            }
            i += 1;
        }
        if a.ifname.is_empty() {
            return Err("--ifname is required (e.g. --ifname enp2s0)".into());
        }
        Ok(a)
    }

    /// Accept `0x1a` / `0X1A` hex or plain decimal, into a u32 we then narrow.
    fn parse_u32(s: &str) -> Result<u32, String> {
        let s = s.trim();
        let r = if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
            u32::from_str_radix(hex, 16)
        } else {
            s.parse::<u32>()
        };
        r.map_err(|_| format!("bad number: {s}"))
    }

    /// Signed value of `bits` width (for i8/i16/i32 writes). Accepts:
    ///   * decimal with optional leading `-` (`-1`, `42`);
    ///   * hex with a leading `-` (`-0x10` = -16);
    ///   * unsigned hex as the two's-complement bit pattern of the target
    ///     width (`0xffff` as i16 = -1, `0x8000` as i16 = -32768).
    ///
    /// Anything outside the width is an error (`0x10000` for i16).
    fn parse_signed(s: &str, bits: u32) -> Result<i64, String> {
        let s = s.trim();
        let (neg, body) = match s.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, s),
        };
        let min = -(1i64 << (bits - 1));
        let max = (1i64 << (bits - 1)) - 1;
        let v = if let Some(hex) = body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
            let raw = u64::from_str_radix(hex, 16).map_err(|_| format!("bad number: {s}"))?;
            if raw >= (1u64 << bits) {
                return Err(format!("{s} does not fit in {bits} bits"));
            }
            let raw = raw as i64;
            if neg {
                -raw
            } else if raw > max {
                // High bit set and no explicit sign: read as two's complement.
                raw - (1i64 << bits)
            } else {
                raw
            }
        } else {
            let mag = body.parse::<i64>().map_err(|_| format!("bad number: {s}"))?;
            if neg {
                -mag
            } else {
                mag
            }
        };
        if v < min || v > max {
            return Err(format!("{s} out of range for i{bits} ({min}..={max})"));
        }
        Ok(v)
    }

    fn is_type(s: &str) -> bool {
        matches!(
            s,
            "u8" | "u16" | "u32" | "i8" | "i16" | "i32" | "bool" | "str" | "hex" | "raw"
        )
    }

    /// Loose bool parser for `w ... bool <v>`: true/false/1/0/on/off/yes/no.
    fn parse_bool(s: &str) -> Result<bool, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "t" | "on" | "yes" | "y" => Ok(true),
            "0" | "false" | "f" | "off" | "no" | "n" => Ok(false),
            other => Err(format!("bad bool '{other}' (use true/false/1/0/on/off)")),
        }
    }

    /// Parse an address token: either `IDX` or `IDX:SUB` (CODESYS `16#..:16#..`
    /// style — both parts 0x-hex or decimal). Returns the embedded sub if the
    /// colon form was used.
    fn parse_addr(tok: &str) -> Result<(u16, Option<u8>), String> {
        if let Some((i, s)) = tok.split_once(':') {
            let index = u16::try_from(parse_u32(i)?).map_err(|_| "idx out of range".to_string())?;
            let sub = u8::try_from(parse_u32(s)?).map_err(|_| "sub out of range".to_string())?;
            Ok((index, Some(sub)))
        } else {
            let index =
                u16::try_from(parse_u32(tok)?).map_err(|_| "idx out of range".to_string())?;
            Ok((index, None))
        }
    }

    pub fn main() {
        let args = match parse_args() {
            Ok(a) => a,
            Err(e) => {
                eprintln!("sdo: {e}");
                std::process::exit(2);
            }
        };
        std::process::exit(smol::block_on(run(args)));
    }

    async fn run(args: Args) -> i32 {
        let ifname = args.ifname.clone();
        // One leaked storage — the process opens exactly one bus. The tx/rx
        // task must outlive this scope, so 'static is required anyway.
        let storage: &'static Storage = Box::leak(Box::new(Storage::new()));
        let (tx, rx, pdu_loop) = match storage.try_split() {
            Ok(parts) => parts,
            Err(_) => {
                eprintln!("sdo: PduStorage already split");
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

        let txrx = match tx_rx_task(&ifname, tx, rx) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("sdo: open {ifname}: {e}");
                return 1;
            }
        };
        if std::thread::Builder::new()
            .name("ecat-txrx".into())
            .spawn(move || {
                if let Err(e) = smol::block_on(txrx) {
                    eprintln!("sdo: tx/rx task exited: {e}");
                }
            })
            .is_err()
        {
            eprintln!("sdo: spawn tx/rx thread failed");
            return 1;
        }

        // Scan → the group lands in PREOP, where CoE SDO already works.
        let group = match maindevice
            .init_single_group::<MAX_SUBDEVICES, MAX_PDI>(ethercat_now)
            .await
        {
            Ok(g) => g,
            Err(e) => {
                eprintln!("sdo: init/scan: {e}");
                return 1;
            }
        };

        list(&group, &maindevice);
        eprintln!(
            "\nsdo: {} subdevice(s), state PREOP. CoE mailbox ready — no process data, no motion.",
            group.len()
        );
        eprintln!("hint: r 0 0x1008 str  (device name)   |   r 0 0x2008:0x14 u16  (an object at sub 0x14)");
        if args.allow_vendor_write {
            eprintln!("vendor objects 0x2000..0x5FFF are WRITABLE (--allow-vendor-write) — on the CT drive a write takes effect immediately.");
        } else {
            eprintln!("vendor objects 0x2000..0x5FFF are write-locked (pass --allow-vendor-write to change drive parameters).");
        }
        eprintln!("type 'help' for commands, 'q' to quit.\n");

        let stdin = std::io::stdin();
        let mut line = String::new();
        loop {
            eprint!("sdo> ");
            let _ = std::io::stderr().flush();
            line.clear();
            match stdin.read_line(&mut line) {
                Ok(0) => break, // EOF (piped input exhausted)
                Ok(_) => {}
                Err(e) => {
                    eprintln!("sdo: stdin: {e}");
                    break;
                }
            }
            let cmd = line.trim();
            if cmd.is_empty() || cmd.starts_with('#') {
                continue;
            }
            let tok: Vec<&str> = cmd.split_whitespace().collect();
            match tok[0] {
                "q" | "quit" | "exit" => break,
                "help" | "?" | "h" => usage(),
                "ls" | "list" => list(&group, &maindevice),
                "r" | "read" => match do_read(&group, &maindevice, &tok).await {
                    Ok(s) => println!("{s}"),
                    Err(e) => eprintln!("sdo: {e}"),
                },
                "w" | "write" => match do_write(&group, &maindevice, &tok, args.allow_vendor_write).await {
                    Ok(s) => println!("{s}"),
                    Err(e) => eprintln!("sdo: {e}"),
                },
                "watch" => {
                    if let Err(e) = do_watch(&group, &maindevice, &tok).await {
                        eprintln!("sdo: {e}");
                    }
                }
                other => eprintln!("sdo: unknown command '{other}' (try 'help')"),
            }
        }
        0
    }

    fn usage() {
        eprintln!(
            "commands (address = '<idx> <sub>' or '<idx>:<sub>'; 0x-hex or decimal):\n  \
             ls                                     list subdevices\n  \
             r <node> <idx[:sub]> [sub] [type]      read  SDO (type: u8 u16 u32 i8 i16 i32 bool str hex; default hex)\n  \
             w <node> <idx[:sub]> [sub] <type> <v>  write SDO (type: u8 u16 u32 i8 i16 i32 bool; signed: -5, -0x10, 0xffff=-1)\n  \
             \x20                                      vendor objects 0x2000..0x5FFF need --allow-vendor-write\n  \
             watch <node> <idx[:sub]> <type> [...]  poll objects, print a line on change (Ctrl-C to stop; 5 failures in a row abort)\n  \
             help / q\n\
             e.g.  r 0 0x2008:0x14 u16   |   watch 0 0x2008:0x14 hex 0x2013:0x0a u16"
        );
    }

    fn list(group: &Group, md: &MainDevice<'static>) {
        eprintln!("subdevices on the chain:");
        for (i, sd) in group.iter(md).enumerate() {
            let id = sd.identity();
            eprintln!(
                "  node {i}: {:#06x}  {}  vendor={:#010x} product={:#010x} rev={:#010x} serial={:#010x}",
                sd.configured_address(),
                sd.name(),
                id.vendor_id,
                id.product_id,
                id.revision,
                id.serial,
            );
        }
    }

    /// Read one object and return just the *value* string (no idx prefix), so
    /// both `r` and `watch` render values identically. `hex` also appends the
    /// ASCII rendering.
    async fn read_value(
        group: &Group,
        md: &MainDevice<'static>,
        node: usize,
        index: u16,
        sub: u8,
        ty: &str,
    ) -> Result<String, String> {
        let sd = group
            .subdevice(md, node)
            .map_err(|_| format!("no such node {node}"))?;
        let m = |e: ethercrab::error::Error| format!("SDO read {index:#06x}:{sub:#04x} failed: {e}");
        Ok(match ty {
            "u8" => {
                let v: u8 = sd.sdo_read(index, sub).await.map_err(m)?;
                format!("{v} (0x{v:02x})")
            }
            "u16" => {
                let v: u16 = sd.sdo_read(index, sub).await.map_err(m)?;
                format!("{v} (0x{v:04x})")
            }
            "u32" => {
                let v: u32 = sd.sdo_read(index, sub).await.map_err(m)?;
                format!("{v} (0x{v:08x})")
            }
            "i8" => {
                let v: i8 = sd.sdo_read(index, sub).await.map_err(m)?;
                format!("{v}")
            }
            "i16" => {
                let v: i16 = sd.sdo_read(index, sub).await.map_err(m)?;
                format!("{v}")
            }
            "i32" => {
                let v: i32 = sd.sdo_read(index, sub).await.map_err(m)?;
                format!("{v}")
            }
            "bool" => {
                // CoE BOOL is one octet over SDO (0/1); read the byte, show the flag.
                let v: u8 = sd.sdo_read(index, sub).await.map_err(m)?;
                format!("{} (0x{v:02x})", v != 0)
            }
            "str" => {
                let v: heapless::String<128> = sd.sdo_read(index, sub).await.map_err(m)?;
                // CoE visible-string objects are fixed-width, NUL-padded.
                format!("{:?}", v.as_str().trim_end_matches('\0'))
            }
            "hex" | "raw" => {
                let v: heapless::Vec<u8, 128> = sd.sdo_read(index, sub).await.map_err(m)?;
                let hex: Vec<String> = v.iter().map(|b| format!("{b:02x}")).collect();
                let ascii: String = v
                    .iter()
                    .map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' })
                    .collect();
                format!("[{}] {} |{}|", v.len(), hex.join(" "), ascii)
            }
            other => return Err(format!("unknown type '{other}' (u8 u16 u32 i8 i16 i32 str hex)")),
        })
    }

    /// From the tokens after `<idx>`, resolve (sub, type). `sub_from_addr` is
    /// Some when the idx token already carried `:sub`. Without a colon, the next
    /// token is the sub if it's a number, or the type if it's a type keyword.
    fn resolve_sub_type<'a>(
        sub_from_addr: Option<u8>,
        t_next: Option<&'a str>,
        t_after: Option<&'a str>,
    ) -> Result<(u8, &'a str), String> {
        if let Some(sub) = sub_from_addr {
            return Ok((sub, t_next.unwrap_or("hex")));
        }
        match t_next {
            None => Ok((0, "hex")),
            Some(t) if is_type(t) => Ok((0, t)),
            Some(s) => {
                let sub =
                    u8::try_from(parse_u32(s)?).map_err(|_| "sub out of range".to_string())?;
                Ok((sub, t_after.unwrap_or("hex")))
            }
        }
    }

    async fn do_read(
        group: &Group,
        md: &MainDevice<'static>,
        tok: &[&str],
    ) -> Result<String, String> {
        // r <node> <idx[:sub]> [sub] [type]
        if tok.len() < 3 {
            return Err("usage: r <node> <idx[:sub]> [sub] [type]".into());
        }
        let node = parse_u32(tok[1])? as usize;
        let (index, sub_from_addr) = parse_addr(tok[2])?;
        let (sub, ty) =
            resolve_sub_type(sub_from_addr, tok.get(3).copied(), tok.get(4).copied())?;
        let val = read_value(group, md, node, index, sub, ty).await?;
        Ok(format!("{index:#06x}:{sub:#04x} = {val}"))
    }

    async fn do_write(
        group: &Group,
        md: &MainDevice<'static>,
        tok: &[&str],
        allow_vendor_write: bool,
    ) -> Result<String, String> {
        // w <node> <idx[:sub]> [sub] <type> <value>
        if tok.len() < 4 {
            return Err("usage: w <node> <idx[:sub]> [sub] <type> <value>".into());
        }
        let node = parse_u32(tok[1])? as usize;
        let (index, sub_from_addr) = parse_addr(tok[2])?;
        let (sub, ty, raw) = if let Some(sub) = sub_from_addr {
            // colon form: w <node> <idx:sub> <type> <value>
            let ty = tok
                .get(3)
                .copied()
                .ok_or_else(|| "usage: w <node> <idx:sub> <type> <value>".to_string())?;
            let raw = tok
                .get(4)
                .copied()
                .ok_or_else(|| "usage: w <node> <idx:sub> <type> <value>".to_string())?;
            (sub, ty, raw)
        } else {
            // separate form: w <node> <idx> <sub> <type> <value>
            let subs = tok
                .get(3)
                .copied()
                .ok_or_else(|| "usage: w <node> <idx> <sub> <type> <value>".to_string())?;
            let sub =
                u8::try_from(parse_u32(subs)?).map_err(|_| "sub out of range".to_string())?;
            let ty = tok
                .get(4)
                .copied()
                .ok_or_else(|| "usage: w <node> <idx> <sub> <type> <value>".to_string())?;
            let raw = tok
                .get(5)
                .copied()
                .ok_or_else(|| "usage: w <node> <idx> <sub> <type> <value>".to_string())?;
            (sub, ty, raw)
        };

        if VENDOR_OBJECTS.contains(&index) && !allow_vendor_write {
            return Err(format!(
                "refusing write to vendor object {index:#06x}:{sub:#04x}: on the CT drive 0x2000..0x5FFF are the live parameter menus \
                 (Pr {}.{sub} here) and a write takes effect immediately even in PREOP (e.g. Pr 6.15 = drive enable) — \
                 restart with --allow-vendor-write to permit",
                index - 0x2000
            ));
        }

        let sd = group
            .subdevice(md, node)
            .map_err(|_| format!("no such node {node}"))?;
        let m =
            |e: ethercrab::error::Error| format!("SDO write {index:#06x}:{sub:#04x} failed: {e}");
        let shown = match ty {
            "u8" => {
                let v = u8::try_from(parse_u32(raw)?).map_err(|_| "value out of range for u8")?;
                sd.sdo_write(index, sub, v).await.map_err(m)?;
                format!("{v} (0x{v:02x})")
            }
            "u16" => {
                let v = u16::try_from(parse_u32(raw)?).map_err(|_| "value out of range for u16")?;
                sd.sdo_write(index, sub, v).await.map_err(m)?;
                format!("{v} (0x{v:04x})")
            }
            "u32" => {
                let v = parse_u32(raw)?;
                sd.sdo_write(index, sub, v).await.map_err(m)?;
                format!("{v} (0x{v:08x})")
            }
            "i8" => {
                let v = i8::try_from(parse_signed(raw, 8)?).map_err(|_| "value out of range for i8")?;
                sd.sdo_write(index, sub, v).await.map_err(m)?;
                format!("{v} (0x{:02x})", v as u8)
            }
            "i16" => {
                let v = i16::try_from(parse_signed(raw, 16)?).map_err(|_| "value out of range for i16")?;
                sd.sdo_write(index, sub, v).await.map_err(m)?;
                format!("{v} (0x{:04x})", v as u16)
            }
            "i32" => {
                let v = i32::try_from(parse_signed(raw, 32)?).map_err(|_| "value out of range for i32")?;
                sd.sdo_write(index, sub, v).await.map_err(m)?;
                format!("{v} (0x{:08x})", v as u32)
            }
            "bool" => {
                let v: u8 = if parse_bool(raw)? { 1 } else { 0 };
                sd.sdo_write(index, sub, v).await.map_err(m)?;
                format!("{} (0x{v:02x})", v != 0)
            }
            other => {
                return Err(format!(
                    "unknown/unsupported write type '{other}' (u8 u16 u32 i8 i16 i32 bool)"
                ))
            }
        };
        Ok(format!("OK wrote {index:#06x}:{sub:#04x} = {shown}"))
    }

    async fn do_watch(
        group: &Group,
        md: &MainDevice<'static>,
        tok: &[&str],
    ) -> Result<(), String> {
        // watch <node> <idx[:sub]> <type> [<idx[:sub]> <type> ...]
        if tok.len() < 4 {
            return Err("usage: watch <node> <idx[:sub]> <type> [more idx type ...]".into());
        }
        let node = parse_u32(tok[1])? as usize;

        // Parse the (object, type) list. Each item is either
        //   idx:sub type      (2 tokens) or
        //   idx sub type       (3 tokens).
        let mut items: Vec<(u16, u8, &str)> = Vec::new();
        let mut i = 2;
        while i < tok.len() {
            let (index, sub_from_addr) = parse_addr(tok[i])?;
            if let Some(sub) = sub_from_addr {
                let ty = tok
                    .get(i + 1)
                    .copied()
                    .ok_or_else(|| format!("watch: missing type after {}", tok[i]))?;
                if !is_type(ty) {
                    return Err(format!("watch: expected a type after {}, got '{ty}'", tok[i]));
                }
                items.push((index, sub, ty));
                i += 2;
            } else {
                let subs = tok
                    .get(i + 1)
                    .copied()
                    .ok_or_else(|| format!("watch: missing sub after {}", tok[i]))?;
                let sub =
                    u8::try_from(parse_u32(subs)?).map_err(|_| "sub out of range".to_string())?;
                let ty = tok
                    .get(i + 2)
                    .copied()
                    .ok_or_else(|| format!("watch: missing type after {} {subs}", tok[i]))?;
                if !is_type(ty) {
                    return Err(format!("watch: expected a type, got '{ty}'"));
                }
                items.push((index, sub, ty));
                i += 3;
            }
        }
        if items.is_empty() {
            return Err("watch: no objects given".into());
        }

        // Arm SIGINT → stop-flag for the duration of the watch only.
        WATCH_STOP.store(false, Ordering::SeqCst);
        // Safety: on_sigint is async-signal-safe (one atomic store).
        unsafe {
            libc::signal(
                libc::SIGINT,
                on_sigint as extern "C" fn(libc::c_int) as *const () as libc::sighandler_t,
            );
        }
        eprintln!(
            "watch: node {node}, {} object(s), ~{}ms period — Ctrl-C to stop. Printing on change:",
            items.len(),
            WATCH_PERIOD_MS
        );

        let start = Instant::now();
        let mut last: Option<Vec<String>> = None;
        let mut cycles = 0usize;
        // A single SDO failure (one 1 s mailbox timeout, a busy drive) must
        // not end a long watch; only a sustained run of failures does.
        const MAX_CONSECUTIVE_FAILURES: usize = 5;
        let mut consecutive_failures = 0usize;
        let mut total_failures = 0usize;
        let outcome = loop {
            if WATCH_STOP.load(Ordering::SeqCst) {
                break Ok(());
            }
            let mut row = Vec::with_capacity(items.len());
            let mut failed = None;
            for &(index, sub, ty) in &items {
                match read_value(group, md, node, index, sub, ty).await {
                    Ok(v) => row.push(v),
                    Err(e) => {
                        failed = Some(e);
                        break;
                    }
                }
            }
            cycles += 1;
            if let Some(e) = failed {
                consecutive_failures += 1;
                total_failures += 1;
                eprintln!(
                    "  t={:7.2}s  watch: {e} ({consecutive_failures}/{MAX_CONSECUTIVE_FAILURES} consecutive — continuing)",
                    start.elapsed().as_secs_f64()
                );
                if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                    break Err(format!(
                        "watch: {MAX_CONSECUTIVE_FAILURES} consecutive SDO failures ({total_failures} total) — giving up"
                    ));
                }
                smol::Timer::after(Duration::from_millis(WATCH_PERIOD_MS)).await;
                continue;
            }
            consecutive_failures = 0;
            if last.as_ref() != Some(&row) {
                let cells: Vec<String> = items
                    .iter()
                    .zip(&row)
                    .map(|((index, sub, _), v)| format!("{index:#06x}:{sub:#04x}={v}"))
                    .collect();
                println!("  t={:7.2}s  {}", start.elapsed().as_secs_f64(), cells.join("   "));
                last = Some(row);
            }
            smol::Timer::after(Duration::from_millis(WATCH_PERIOD_MS)).await;
        };

        // Restore default SIGINT (terminate) for the prompt.
        unsafe {
            libc::signal(libc::SIGINT, libc::SIG_DFL);
        }
        eprintln!("watch: stopped after {cycles} poll cycle(s), {total_failures} failed");
        outcome
    }
}
