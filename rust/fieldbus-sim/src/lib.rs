//! Simulation backend: a first-order axis model behind the `fieldbus-api`
//! seam, so the whole daemon⇄bridge⇄HMI path runs on WSL/CI without any
//! hardware.
//!
//! Deliberately declares the *same* capability profile the EtherCAT CSP
//! backend will (`HardRt` + `CyclicTrajectory`), so `motion-core` exercises
//! the exact strategy — per-cycle trajectory points — that reaches real
//! drives in Phase 3.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fieldbus_api::{
    AcyclicAccess, AxisCapability, AxisId, AxisIn, AxisOut, BusEvent, BusState, CycleClass,
    DriveStatus, ExchangeStatus, Fieldbus, FieldbusError, OpModes, ParamAddr, Setpoint,
    SetpointKind,
};

#[derive(Clone, Copy, Debug)]
pub struct SimConfig {
    pub axes: usize,
    /// Nominal exchange period; also the model integration step.
    pub cycle: Duration,
    /// First-order tracking time constant (s) — how sluggish the "drive" is.
    pub tau: f64,
    /// Cycles spent in `Enabling` before the power stage reports ready.
    pub enable_delay_cycles: u32,
    /// Speed of the simulated homing motion (user units/s).
    pub home_vel: f64,
}

impl Default for SimConfig {
    fn default() -> Self {
        SimConfig {
            axes: 4,
            cycle: Duration::from_millis(2),
            tau: 0.03,
            enable_delay_cycles: 5,
            home_vel: 50.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct SimAxis {
    pos: f64,
    vel: f64,
    enabled: bool,
    enable_cnt: u32,
    fault_code: u32,
    homed: bool,
    /// Optional hardware limit switch positions (test hook).
    neg_limit_at: Option<f64>,
    pos_limit_at: Option<f64>,
}

pub struct SimBackend {
    cfg: SimConfig,
    state: BusState,
    axes: Vec<SimAxis>,
    events: VecDeque<BusEvent>,
    params: SharedParams,
}

type SharedParams = Arc<Mutex<BTreeMap<(usize, ParamKey), Vec<u8>>>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ParamKey {
    Object(u16, u8),
    Register(u16),
}

impl From<ParamAddr> for ParamKey {
    fn from(a: ParamAddr) -> Self {
        match a {
            ParamAddr::Object { index, sub } => ParamKey::Object(index, sub),
            ParamAddr::Register { addr } => ParamKey::Register(addr),
        }
    }
}

impl SimBackend {
    pub fn new(cfg: SimConfig) -> SimBackend {
        SimBackend {
            axes: vec![SimAxis::default(); cfg.axes],
            cfg,
            state: BusState::Init,
            events: VecDeque::new(),
            params: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// Test hook: make the drive report a fault.
    pub fn inject_fault(&mut self, axis: usize, code: u32) {
        self.axes[axis].fault_code = code;
        self.events.push_back(BusEvent::AxisFault {
            axis: AxisId(axis),
            code,
        });
    }

    /// Test hook: place hardware limit switches.
    pub fn set_limits(&mut self, axis: usize, neg_at: Option<f64>, pos_at: Option<f64>) {
        self.axes[axis].neg_limit_at = neg_at;
        self.axes[axis].pos_limit_at = pos_at;
    }

    pub fn axis_pos(&self, axis: usize) -> f64 {
        self.axes[axis].pos
    }

    fn step_axis(ax: &mut SimAxis, out: &AxisOut, cfg: &SimConfig) {
        let dt = cfg.cycle.as_secs_f64();
        // first-order tracking gain
        let k = dt / (cfg.tau + dt);

        if out.fault_reset && ax.fault_code != 0 {
            ax.fault_code = 0;
        }

        if !out.enable {
            ax.enabled = false;
            ax.enable_cnt = 0;
            ax.vel = 0.0; // power stage off → brake engages
            return;
        }
        if ax.fault_code != 0 {
            ax.enabled = false;
            ax.enable_cnt = 0;
            ax.vel = 0.0;
            return;
        }
        if !ax.enabled {
            ax.enable_cnt += 1;
            if ax.enable_cnt >= cfg.enable_delay_cycles {
                ax.enabled = true;
            }
            return;
        }

        match out.setpoint {
            Setpoint::Hold => {
                ax.vel += (0.0 - ax.vel) * k;
                if ax.vel.abs() < 1e-6 {
                    ax.vel = 0.0;
                }
                ax.pos += ax.vel * dt;
            }
            Setpoint::CyclicPosition { pos, .. } => {
                let new_pos = ax.pos + (pos - ax.pos) * k;
                ax.vel = (new_pos - ax.pos) / dt;
                ax.pos = new_pos;
            }
            Setpoint::CyclicVelocity { vel } => {
                ax.vel += (vel - ax.vel) * k;
                ax.pos += ax.vel * dt;
            }
            Setpoint::Home => {
                let step = cfg.home_vel * dt;
                if ax.pos.abs() <= step {
                    ax.pos = 0.0;
                    ax.vel = 0.0;
                    ax.homed = true;
                } else {
                    let dir = if ax.pos > 0.0 { -1.0 } else { 1.0 };
                    ax.vel = dir * cfg.home_vel;
                    ax.pos += ax.vel * dt;
                }
            }
            // Sim declares CyclicTrajectory, so motion-core never sends
            // these; track toward the target anyway to stay forgiving.
            Setpoint::TargetPosition { pos, .. } => {
                let new_pos = ax.pos + (pos - ax.pos) * k;
                ax.vel = (new_pos - ax.pos) / dt;
                ax.pos = new_pos;
            }
            Setpoint::TargetVelocity { vel, .. } => {
                ax.vel += (vel - ax.vel) * k;
                ax.pos += ax.vel * dt;
            }
        }
    }

    fn drive_status(ax: &SimAxis, out: &AxisOut) -> DriveStatus {
        if ax.fault_code != 0 {
            DriveStatus::Fault
        } else if ax.enabled {
            DriveStatus::Enabled
        } else if out.enable {
            DriveStatus::Enabling
        } else {
            DriveStatus::Disabled
        }
    }
}

impl Fieldbus for SimBackend {
    type Acyclic = SimAcyclic;

    fn axis_count(&self) -> usize {
        self.axes.len()
    }

    fn capability(&self, _axis: AxisId) -> AxisCapability {
        AxisCapability {
            cycle: CycleClass::HardRt {
                cycle: self.cfg.cycle,
            },
            setpoint: SetpointKind::CyclicTrajectory,
            modes: OpModes {
                position: true,
                velocity: true,
                torque: false,
                homing: true,
            },
        }
    }

    fn bus_state(&self) -> BusState {
        self.state
    }

    async fn start(&mut self) -> Result<(), FieldbusError> {
        self.state = BusState::Op;
        self.events.push_back(BusEvent::BusStateChanged(BusState::Op));
        for i in 0..self.axes.len() {
            self.events.push_back(BusEvent::AxisOnline(AxisId(i)));
        }
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), FieldbusError> {
        self.state = BusState::Init;
        self.events
            .push_back(BusEvent::BusStateChanged(BusState::Init));
        Ok(())
    }

    async fn exchange(
        &mut self,
        outs: &[AxisOut],
        ins: &mut [AxisIn],
    ) -> Result<ExchangeStatus, FieldbusError> {
        if self.state != BusState::Op {
            return Err(FieldbusError::InvalidState(self.state));
        }
        for (i, ax) in self.axes.iter_mut().enumerate() {
            let out = &outs[i];
            Self::step_axis(ax, out, &self.cfg);
            ins[i] = AxisIn {
                act_pos: ax.pos,
                act_vel: ax.vel,
                drive: Self::drive_status(ax, out),
                fault_code: ax.fault_code,
                pos_limit: ax.pos_limit_at.is_some_and(|l| ax.pos >= l),
                neg_limit: ax.neg_limit_at.is_some_and(|l| ax.pos <= l),
                homed: ax.homed,
            };
        }
        Ok(ExchangeStatus {
            all_axes_responding: true,
            inputs_fresh: true,
        })
    }

    fn poll_event(&mut self) -> Option<BusEvent> {
        self.events.pop_front()
    }

    fn acyclic(&self) -> SimAcyclic {
        SimAcyclic {
            params: Arc::clone(&self.params),
        }
    }
}

/// In-memory parameter store standing in for SDO / register access.
pub struct SimAcyclic {
    params: SharedParams,
}

impl AcyclicAccess for SimAcyclic {
    async fn read(
        &mut self,
        axis: AxisId,
        addr: ParamAddr,
        buf: &mut [u8],
    ) -> Result<usize, FieldbusError> {
        let map = self.params.lock().unwrap();
        match map.get(&(axis.0, addr.into())) {
            Some(v) => {
                let n = v.len().min(buf.len());
                buf[..n].copy_from_slice(&v[..n]);
                Ok(n)
            }
            None => Err(FieldbusError::Acyclic { code: 0x0602_0000 }), // SDO "object does not exist"
        }
    }

    async fn write(
        &mut self,
        axis: AxisId,
        addr: ParamAddr,
        data: &[u8],
    ) -> Result<(), FieldbusError> {
        self.params
            .lock()
            .unwrap()
            .insert((axis.0, addr.into()), data.to_vec());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block_on<T>(fut: impl core::future::Future<Output = T>) -> T {
        // The sim's futures are always immediately ready; poll once.
        use core::pin::pin;
        use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
        fn noop(_: *const ()) {}
        fn clone(_: *const ()) -> RawWaker {
            RawWaker::new(core::ptr::null(), &VTABLE)
        }
        static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
        let waker = unsafe { Waker::from_raw(RawWaker::new(core::ptr::null(), &VTABLE)) };
        let mut cx = Context::from_waker(&waker);
        match pin!(fut).as_mut().poll(&mut cx) {
            Poll::Ready(v) => v,
            Poll::Pending => panic!("sim future must be immediately ready"),
        }
    }

    #[test]
    fn enable_sequence_and_csp_tracking() {
        let mut bus = SimBackend::new(SimConfig {
            axes: 1,
            ..Default::default()
        });
        block_on(bus.start()).unwrap();
        assert_eq!(bus.bus_state(), BusState::Op);

        let mut ins = [AxisIn::default(); 1];
        let mut outs = [AxisOut::default(); 1];

        block_on(bus.exchange(&outs, &mut ins)).unwrap();
        assert_eq!(ins[0].drive, DriveStatus::Disabled);

        outs[0].enable = true;
        block_on(bus.exchange(&outs, &mut ins)).unwrap();
        assert_eq!(ins[0].drive, DriveStatus::Enabling);
        for _ in 0..10 {
            block_on(bus.exchange(&outs, &mut ins)).unwrap();
        }
        assert_eq!(ins[0].drive, DriveStatus::Enabled);

        // CSP stream: ramp the commanded position, the model must track it.
        let mut cmd_pos = 0.0;
        for _ in 0..2000 {
            cmd_pos += 10.0 * 0.002; // 10 units/s
            outs[0].setpoint = Setpoint::CyclicPosition {
                pos: cmd_pos,
                vel_ff: 10.0,
            };
            block_on(bus.exchange(&outs, &mut ins)).unwrap();
        }
        assert!(
            (ins[0].act_pos - cmd_pos).abs() < 0.5,
            "tracking error too large: cmd={cmd_pos} act={}",
            ins[0].act_pos
        );
        assert!((ins[0].act_vel - 10.0).abs() < 0.5);
    }

    #[test]
    fn fault_inject_and_reset() {
        let mut bus = SimBackend::new(SimConfig {
            axes: 1,
            ..Default::default()
        });
        block_on(bus.start()).unwrap();
        let mut ins = [AxisIn::default(); 1];
        let mut outs = [AxisOut::default(); 1];
        outs[0].enable = true;
        for _ in 0..10 {
            block_on(bus.exchange(&outs, &mut ins)).unwrap();
        }
        assert_eq!(ins[0].drive, DriveStatus::Enabled);

        bus.inject_fault(0, 0x7500);
        block_on(bus.exchange(&outs, &mut ins)).unwrap();
        assert_eq!(ins[0].drive, DriveStatus::Fault);
        assert_eq!(ins[0].fault_code, 0x7500);
        assert!(matches!(
            bus.poll_event(),
            Some(BusEvent::BusStateChanged(_)) | Some(BusEvent::AxisOnline(_))
        ));

        outs[0].fault_reset = true;
        block_on(bus.exchange(&outs, &mut ins)).unwrap();
        assert_eq!(ins[0].fault_code, 0);
        // re-enable sequence runs again
        for _ in 0..10 {
            block_on(bus.exchange(&outs, &mut ins)).unwrap();
        }
        assert_eq!(ins[0].drive, DriveStatus::Enabled);
    }

    #[test]
    fn acyclic_round_trip() {
        let bus = SimBackend::new(SimConfig::default());
        let mut ac = bus.acyclic();
        let addr = ParamAddr::Object {
            index: 0x6098,
            sub: 0,
        };
        block_on(ac.write(AxisId(0), addr, &[33])).unwrap();
        let mut buf = [0u8; 4];
        let n = block_on(ac.read(AxisId(0), addr, &mut buf)).unwrap();
        assert_eq!((n, buf[0]), (1, 33));
        assert!(block_on(ac.read(AxisId(1), addr, &mut buf)).is_err());
    }
}
