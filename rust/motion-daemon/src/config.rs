//! Configuration: an optional `key = value` file plus CLI overrides.
//! Deliberately dependency-free — the daemon runs on a locked-down
//! industrial PC, a TOML parser buys nothing here.
//!
//! Every value is validated where it is parsed (so a bad line reports its
//! line number): kinematic limits must be finite and positive (a `stop_dec`
//! of 0 would make EMS inert — the ramp never reaches zero), `scale` must be
//! finite and non-zero (it is a divisor), `cycle_ms` is checked *before*
//! `Duration::from_secs_f64` sees it (NaN/negative panic there), and the
//! counts are parsed as integers instead of truncating a float.

use std::str::FromStr;
use std::time::Duration;

use motion_core::AxisParams;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Sim,
    Ethercat,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub backend: Backend,
    /// Served axes (1..=4 — the shm layout carries exactly 4 slots; unserved
    /// slots publish zeros).
    pub axes: usize,
    pub cycle: Duration,
    pub params: AxisParams,
    /// Sim model tuning.
    pub sim_tau: f64,
    /// EtherCAT NIC.
    pub ifname: String,
    /// Drive increments per user unit (EtherCAT backend).
    pub scale: f64,
    /// Distributed clocks SYNC0 (EtherCAT backend).
    pub ecat_dc: bool,
    /// Chain position of the first axis drive (couplers before it shift this).
    pub first_axis_subdevice: usize,
    /// Ring window of the /dev/shm trace segment, seconds (0 = trace off).
    pub trace_seconds: u64,
    /// Command dead-man: a latched jog word older than this (no new command
    /// message from the bridge) has its jog bits stripped. `None` = off.
    /// Clients refresh jog every 250 ms and the bridge's own watchdog clears
    /// at 500 ms, so 1 s only ever fires when the bridge itself is gone.
    pub cmd_timeout: Option<Duration>,
    /// Upper bound on the EMS-ramp phase of a graceful shutdown; after it
    /// the drives are disabled whether or not every axis stands still.
    pub shutdown_timeout: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            backend: Backend::Sim,
            axes: 4,
            cycle: Duration::from_millis(2),
            params: AxisParams::default(),
            sim_tau: 0.03,
            ifname: String::new(),
            scale: 10_000.0,
            ecat_dc: true,
            first_axis_subdevice: 0,
            trace_seconds: 60,
            cmd_timeout: Some(Duration::from_millis(1000)),
            shutdown_timeout: Duration::from_millis(2000),
        }
    }
}

pub fn parse(args: impl Iterator<Item = String>) -> Result<Config, String> {
    let mut cfg = Config::default();
    let mut args = args.skip(1).peekable();

    // First pass argv for --config, apply the file, then let the remaining
    // flags override it.
    let argv: Vec<String> = args.by_ref().collect();
    if let Some(i) = argv.iter().position(|a| a == "--config") {
        let path = argv
            .get(i + 1)
            .ok_or_else(|| "--config needs a path".to_string())?;
        let text =
            std::fs::read_to_string(path).map_err(|e| format!("read config {path}: {e}"))?;
        apply_file(&mut cfg, &text)?;
    }

    let mut it = argv.iter().enumerate();
    while let Some((i, arg)) = it.next() {
        let val = |name: &str| -> Result<&String, String> {
            argv.get(i + 1)
                .ok_or_else(|| format!("{name} needs a value"))
        };
        // `--flag-name` ↔ `flag_name` config key; the file and the CLI share
        // one parser so the validation rules cannot drift apart.
        let key = match arg.as_str() {
            "--config" => {
                let _ = val("--config")?; // consumed in the first pass
                it.next();
                continue;
            }
            "--backend" => "backend",
            "--axes" => "axes",
            "--cycle-ms" => "cycle_ms",
            "--ifname" => "ifname",
            "--trace-seconds" => "trace_seconds",
            "--cmd-timeout-ms" => "cmd_timeout_ms",
            "--shutdown-timeout-ms" => "shutdown_timeout_ms",
            "--help" | "-h" => return Err(USAGE.to_string()),
            other => return Err(format!("unknown flag {other}\n{USAGE}")),
        };
        set(&mut cfg, key, val(arg)?)?;
        it.next();
    }

    if cfg.axes == 0 || cfg.axes > 4 {
        return Err(format!("axes must be 1..=4, got {}", cfg.axes));
    }
    if cfg.cycle < Duration::from_micros(250) || cfg.cycle > Duration::from_secs(1) {
        return Err(format!("cycle out of range: {:?}", cfg.cycle));
    }
    // 600 s @ 250 µs is already a 1 GiB ring — past that you want files,
    // not /dev/shm.
    if cfg.trace_seconds > 600 {
        return Err(format!(
            "trace_seconds must be 0..=600, got {}",
            cfg.trace_seconds
        ));
    }
    Ok(cfg)
}

// A `\` line continuation strips the next line's leading whitespace, so the
// indentation of continued lines sits *before* each `\n\`.
pub const USAGE: &str = "usage: motion-daemon [--config FILE] [--backend sim|ethercat] \
[--axes N] [--cycle-ms MS] [--ifname IF] [--trace-seconds S]\n                     \
[--cmd-timeout-ms MS] [--shutdown-timeout-ms MS]\n\
config file keys: backend, axes, cycle_ms, ifname, max_vel, acc, dec, stop_dec, sim_tau_ms,\n                  \
scale, ecat_dc, first_axis_subdevice, trace_seconds,\n                  \
cmd_timeout_ms (0 = dead-man off), shutdown_timeout_ms";

fn apply_file(cfg: &mut Config, text: &str) -> Result<(), String> {
    for (lineno, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let (k, v) = line
            .split_once('=')
            .ok_or_else(|| format!("config line {}: expected key = value", lineno + 1))?;
        set(cfg, k.trim(), v.trim()).map_err(|e| format!("config line {}: {e}", lineno + 1))?;
    }
    Ok(())
}

/// A finite number (`nan`/`inf` parse fine as f64 — reject them here).
fn finite(key: &str, value: &str) -> Result<f64, String> {
    let v = value
        .parse::<f64>()
        .map_err(|_| format!("{key}: not a number: {value}"))?;
    if !v.is_finite() {
        return Err(format!("{key}: must be finite, got {value}"));
    }
    Ok(v)
}

fn positive(key: &str, value: &str) -> Result<f64, String> {
    let v = finite(key, value)?;
    if v <= 0.0 {
        return Err(format!("{key}: must be > 0, got {value}"));
    }
    Ok(v)
}

fn non_negative(key: &str, value: &str) -> Result<f64, String> {
    let v = finite(key, value)?;
    if v < 0.0 {
        return Err(format!("{key}: must be >= 0, got {value}"));
    }
    Ok(v)
}

fn integer<T: FromStr>(key: &str, value: &str) -> Result<T, String> {
    value
        .parse::<T>()
        .map_err(|_| format!("{key}: not an integer: {value}"))
}

/// Milliseconds → `Duration`, refusing what `from_secs_f64` would panic on
/// (already excluded by `non_negative`, but absurdly large values overflow).
fn millis(key: &str, ms: f64) -> Result<Duration, String> {
    Duration::try_from_secs_f64(ms / 1000.0).map_err(|_| format!("{key}: out of range: {ms}"))
}

fn set(cfg: &mut Config, key: &str, value: &str) -> Result<(), String> {
    match key {
        "backend" => {
            cfg.backend = match value {
                "sim" => Backend::Sim,
                "ethercat" => Backend::Ethercat,
                _ => return Err(format!("backend must be sim|ethercat, got {value}")),
            }
        }
        "axes" => cfg.axes = integer(key, value)?,
        "cycle_ms" => cfg.cycle = millis(key, positive(key, value)?)?,
        "ifname" => cfg.ifname = value.to_string(),
        "max_vel" => cfg.params.max_vel = positive(key, value)?,
        "acc" => cfg.params.acc = positive(key, value)?,
        "dec" => cfg.params.dec = positive(key, value)?,
        "stop_dec" => cfg.params.stop_dec = positive(key, value)?,
        "sim_tau_ms" => cfg.sim_tau = non_negative(key, value)? / 1000.0,
        "scale" => {
            let v = finite(key, value)?;
            if v == 0.0 {
                return Err(format!("{key}: must be non-zero"));
            }
            cfg.scale = v;
        }
        "ecat_dc" => {
            cfg.ecat_dc = match value {
                "true" | "1" => true,
                "false" | "0" => false,
                _ => return Err(format!("{key}: must be true|false, got {value}")),
            }
        }
        "first_axis_subdevice" => cfg.first_axis_subdevice = integer(key, value)?,
        "trace_seconds" => cfg.trace_seconds = integer(key, value)?,
        "cmd_timeout_ms" => {
            let ms = non_negative(key, value)?;
            cfg.cmd_timeout = if ms == 0.0 {
                None
            } else {
                Some(millis(key, ms)?)
            };
        }
        "shutdown_timeout_ms" => cfg.shutdown_timeout = millis(key, non_negative(key, value)?)?,
        _ => return Err(format!("unknown key {key}")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(s: &str) -> impl Iterator<Item = String> + '_ {
        std::iter::once("motion-daemon".to_string()).chain(s.split_whitespace().map(String::from))
    }

    #[test]
    fn defaults_and_overrides() {
        let cfg = parse(argv("")).unwrap();
        assert_eq!(cfg.backend, Backend::Sim);
        assert_eq!(cfg.axes, 4);
        assert_eq!(cfg.cmd_timeout, Some(Duration::from_millis(1000)));
        assert_eq!(cfg.shutdown_timeout, Duration::from_millis(2000));

        let cfg = parse(argv("--backend sim --axes 2 --cycle-ms 5")).unwrap();
        assert_eq!(cfg.axes, 2);
        assert_eq!(cfg.cycle, Duration::from_millis(5));
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse(argv("--backend plc")).is_err());
        assert!(parse(argv("--axes 9")).is_err());
        assert!(parse(argv("--frobnicate")).is_err());
        assert!(parse(argv("--trace-seconds 601")).is_err());
        assert!(parse(argv("--axes")).is_err(), "flag without value");
    }

    /// The values that used to panic (`Duration::from_secs_f64`), divide by
    /// zero, silently truncate, or disarm EMS.
    #[test]
    fn rejects_non_finite_zero_and_fractional_counts() {
        for bad in ["nan", "-1", "0", "inf", "1e300", "abc"] {
            assert!(parse(argv(&format!("--cycle-ms {bad}"))).is_err(), "cycle_ms {bad}");
        }
        for bad in ["2.7", "-1", "nan", "1e1"] {
            assert!(parse(argv(&format!("--axes {bad}"))).is_err(), "axes {bad}");
        }
        for bad in ["1.5", "-3"] {
            assert!(
                parse(argv(&format!("--trace-seconds {bad}"))).is_err(),
                "trace_seconds {bad}"
            );
        }

        let mut cfg = Config::default();
        for key in ["max_vel", "acc", "dec", "stop_dec"] {
            for bad in ["0", "-5", "nan", "inf"] {
                assert!(set(&mut cfg, key, bad).is_err(), "{key} = {bad}");
            }
            assert!(set(&mut cfg, key, "100").is_ok());
        }
        for bad in ["0", "nan", "inf", "-inf"] {
            assert!(set(&mut cfg, "scale", bad).is_err(), "scale = {bad}");
        }
        assert!(set(&mut cfg, "scale", "-10000").is_ok(), "negative scale flips direction");
        assert!(set(&mut cfg, "first_axis_subdevice", "1.5").is_err());
        assert!(set(&mut cfg, "sim_tau_ms", "-1").is_err());
        assert!(set(&mut cfg, "sim_tau_ms", "0").is_ok());
        assert!(set(&mut cfg, "ecat_dc", "maybe").is_err());
        assert!(set(&mut cfg, "ecat_dc", "0").is_ok());
        assert!(!cfg.ecat_dc);

        // Bad file lines report their line number.
        let err = apply_file(&mut cfg, "axes = 2\nstop_dec = 0\n").unwrap_err();
        assert!(err.starts_with("config line 2:"), "{err}");
    }

    #[test]
    fn trace_seconds_flag() {
        assert_eq!(parse(argv("")).unwrap().trace_seconds, 60);
        assert_eq!(parse(argv("--trace-seconds 0")).unwrap().trace_seconds, 0);
        assert_eq!(
            parse(argv("--trace-seconds 120")).unwrap().trace_seconds,
            120
        );
    }

    #[test]
    fn timeout_flags() {
        let cfg = parse(argv("--cmd-timeout-ms 0")).unwrap();
        assert_eq!(cfg.cmd_timeout, None, "0 disables the dead-man");
        let cfg = parse(argv("--cmd-timeout-ms 250 --shutdown-timeout-ms 500")).unwrap();
        assert_eq!(cfg.cmd_timeout, Some(Duration::from_millis(250)));
        assert_eq!(cfg.shutdown_timeout, Duration::from_millis(500));
        assert!(parse(argv("--cmd-timeout-ms -1")).is_err());
        assert!(parse(argv("--cmd-timeout-ms nan")).is_err());
        assert!(parse(argv("--shutdown-timeout-ms inf")).is_err());
        assert!(parse(argv("--shutdown-timeout-ms -0.5")).is_err());
    }

    #[test]
    fn config_file_then_cli_override() {
        let mut cfg = Config::default();
        apply_file(
            &mut cfg,
            "# comment\nbackend = sim\naxes = 3\nmax_vel = 250 # trailing\n\
             cmd_timeout_ms = 0\nshutdown_timeout_ms = 3000\n",
        )
        .unwrap();
        assert_eq!(cfg.axes, 3);
        assert_eq!(cfg.params.max_vel, 250.0);
        assert_eq!(cfg.cmd_timeout, None);
        assert_eq!(cfg.shutdown_timeout, Duration::from_millis(3000));
    }
}
