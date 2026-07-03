//! Configuration: an optional `key = value` file plus CLI overrides.
//! Deliberately dependency-free — the daemon runs on a locked-down
//! industrial PC, a TOML parser buys nothing here.

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
        match arg.as_str() {
            "--config" => {
                let _ = val("--config")?; // consumed in the first pass
                it.next();
            }
            "--backend" => {
                set(&mut cfg, "backend", val("--backend")?)?;
                it.next();
            }
            "--axes" => {
                set(&mut cfg, "axes", val("--axes")?)?;
                it.next();
            }
            "--cycle-ms" => {
                set(&mut cfg, "cycle_ms", val("--cycle-ms")?)?;
                it.next();
            }
            "--ifname" => {
                set(&mut cfg, "ifname", val("--ifname")?)?;
                it.next();
            }
            "--help" | "-h" => return Err(USAGE.to_string()),
            other => return Err(format!("unknown flag {other}\n{USAGE}")),
        }
    }

    if cfg.axes == 0 || cfg.axes > 4 {
        return Err(format!("axes must be 1..=4, got {}", cfg.axes));
    }
    if cfg.cycle < Duration::from_micros(250) || cfg.cycle > Duration::from_secs(1) {
        return Err(format!("cycle out of range: {:?}", cfg.cycle));
    }
    Ok(cfg)
}

pub const USAGE: &str = "usage: motion-daemon [--config FILE] [--backend sim|ethercat] \
[--axes N] [--cycle-ms MS] [--ifname IF]\n\
config file keys: backend, axes, cycle_ms, ifname, max_vel, acc, dec, stop_dec, sim_tau_ms,\n\
                  scale, ecat_dc, first_axis_subdevice";

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

fn set(cfg: &mut Config, key: &str, value: &str) -> Result<(), String> {
    fn num(key: &str, value: &str) -> Result<f64, String> {
        value
            .parse::<f64>()
            .map_err(|_| format!("{key}: not a number: {value}"))
    }
    match key {
        "backend" => {
            cfg.backend = match value {
                "sim" => Backend::Sim,
                "ethercat" => Backend::Ethercat,
                _ => return Err(format!("backend must be sim|ethercat, got {value}")),
            }
        }
        "axes" => cfg.axes = num(key, value)? as usize,
        "cycle_ms" => cfg.cycle = Duration::from_secs_f64(num(key, value)? / 1000.0),
        "ifname" => cfg.ifname = value.to_string(),
        "max_vel" => cfg.params.max_vel = num(key, value)?,
        "acc" => cfg.params.acc = num(key, value)?,
        "dec" => cfg.params.dec = num(key, value)?,
        "stop_dec" => cfg.params.stop_dec = num(key, value)?,
        "sim_tau_ms" => cfg.sim_tau = num(key, value)? / 1000.0,
        "scale" => cfg.scale = num(key, value)?,
        "ecat_dc" => cfg.ecat_dc = value == "true" || value == "1",
        "first_axis_subdevice" => cfg.first_axis_subdevice = num(key, value)? as usize,
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

        let cfg = parse(argv("--backend sim --axes 2 --cycle-ms 5")).unwrap();
        assert_eq!(cfg.axes, 2);
        assert_eq!(cfg.cycle, Duration::from_millis(5));
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse(argv("--backend plc")).is_err());
        assert!(parse(argv("--axes 9")).is_err());
        assert!(parse(argv("--frobnicate")).is_err());
    }

    #[test]
    fn config_file_then_cli_override() {
        let mut cfg = Config::default();
        apply_file(
            &mut cfg,
            "# comment\nbackend = sim\naxes = 3\nmax_vel = 250 # trailing\n",
        )
        .unwrap();
        assert_eq!(cfg.axes, 3);
        assert_eq!(cfg.params.max_vel, 250.0);
    }
}
