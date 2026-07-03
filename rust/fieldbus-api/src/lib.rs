//! Bus-agnostic fieldbus abstraction for the motion daemon.
//!
//! `motion-core` talks to drives exclusively through the [`Fieldbus`] trait.
//! Concrete buses live in adapter crates (`fieldbus-ethercat`, `fieldbus-sim`,
//! a future `fieldbus-modbus`) and their native types must not leak past the
//! adapter boundary — this crate defines the only vocabulary shared between
//! the motion core and any bus.
//!
//! The abstraction covers two very different worlds and must keep doing so:
//!
//! * **EtherCAT CSP**: hard-RT, DC-synchronised, a fresh trajectory point is
//!   exchanged every cycle (1 ms), the trajectory generator runs in
//!   `motion-core`.
//! * **Modbus TCP/RTU**: soft, 10–100 ms register polling, the daemon only
//!   forwards a target + limits and the drive/inverter profiles internally.
//!
//! Which world an axis lives in is *data*, not code: [`AxisCapability`]
//! describes it per axis, and `motion-core` picks its strategy from that.
//! See `rust/README.md` for the design rationale (ADR-1 … ADR-6).

#![forbid(unsafe_code)]

use core::fmt;
use core::time::Duration;

// ─── Identity ────────────────────────────────────────────────────────────────

/// Index of an axis slot on *one bus instance* (0-based, dense).
///
/// A daemon may own several `Fieldbus` instances (e.g. two EtherCAT
/// MainDevices bound to two NICs). The mapping from the machine-global axis
/// number (shm `Axes[i]`) to `(bus instance, AxisId)` is configuration owned
/// by `motion-daemon`; nothing in this crate assumes a single bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AxisId(pub usize);

impl fmt::Display for AxisId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "axis{}", self.0)
    }
}

// ─── Capability ──────────────────────────────────────────────────────────────

/// Cycle timing class of an axis (usually uniform per backend).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CycleClass {
    /// Hard real-time cyclic exchange; `exchange()` must be called every
    /// `cycle` and completes one bus transaction (EtherCAT PDO tx/rx,
    /// DC-synchronised). The adapter's `exchange()` is allocation-free.
    HardRt { cycle: Duration },
    /// Soft polling (Modbus TCP/RTU, …). `exchange()` may be called faster
    /// than `nominal`; the adapter rate-limits internally and returns cached
    /// inputs between polls (`ExchangeStatus::inputs_fresh == false`).
    SoftPoll { nominal: Duration },
}

/// How the axis consumes motion demands — decides where trajectories run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetpointKind {
    /// The backend consumes a fresh setpoint every cycle (CSP/CSV).
    /// `motion-core` runs the trajectory generator (rsruckig) and streams
    /// [`Setpoint::CyclicPosition`] / [`Setpoint::CyclicVelocity`].
    CyclicTrajectory,
    /// The backend forwards a target + kinematic limits to the drive and the
    /// drive profiles internally (Modbus inverter, drive-internal
    /// positioning). `motion-core` sends [`Setpoint::TargetPosition`] /
    /// [`Setpoint::TargetVelocity`] once per command and then monitors.
    TargetForwarding,
}

/// Operating modes an axis supports. `motion-core` refuses commands the axis
/// cannot express (e.g. `moveAbs` on a velocity-only inverter).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OpModes {
    pub position: bool,
    pub velocity: bool,
    pub torque: bool,
    pub homing: bool,
}

/// Per-axis capability, queried once after [`Fieldbus::start`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AxisCapability {
    pub cycle: CycleClass,
    pub setpoint: SetpointKind,
    pub modes: OpModes,
}

// ─── Cyclic process image ────────────────────────────────────────────────────

/// Drive power/health state, normalised across buses.
///
/// CiA 402 mapping (done *inside* the EtherCAT adapter, see ADR-4):
/// `Not ready/Switch on disabled/Ready to switch on/Switched on` → `Disabled`,
/// intermediate transitions after an enable request → `Enabling`,
/// `Operation enabled` → `Enabled`, `Quick stop active` → `QuickStop`,
/// `Fault reaction active/Fault` → `Fault`.
/// Modbus mapping: not connected → `Offline`, run bit off → `Disabled`,
/// run bit on → `Enabled`, fault register ≠ 0 → `Fault`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DriveStatus {
    /// Not on the bus / no valid inputs yet.
    #[default]
    Offline,
    /// Powered but not enabled; will not follow setpoints.
    Disabled,
    /// Enable requested, power stage not yet operational.
    Enabling,
    /// Operational — follows setpoints.
    Enabled,
    /// Quick-stop ramp active (still under drive control).
    QuickStop,
    /// Drive fault; `AxisIn::fault_code` carries the backend-native code.
    Fault,
}

/// Per-axis inputs, backend → motion-core, in user units (the adapter owns
/// increment↔unit scaling).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AxisIn {
    pub act_pos: f64,
    pub act_vel: f64,
    pub drive: DriveStatus,
    /// Backend-native fault/error code, 0 = none (402: error code 0x603F;
    /// Modbus: fault register value).
    pub fault_code: u32,
    /// Hardware limit switches as far as the bus can see them.
    pub pos_limit: bool,
    pub neg_limit: bool,
    /// Axis has a valid home reference.
    pub homed: bool,
}

/// Motion demand for one axis for one cycle.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Setpoint {
    /// No motion demand — hold position (or keep the drive's last state).
    #[default]
    Hold,
    /// CSP: position for *this* cycle plus velocity feed-forward.
    /// Only meaningful on `SetpointKind::CyclicTrajectory` axes.
    CyclicPosition { pos: f64, vel_ff: f64 },
    /// CSV: velocity for this cycle.
    CyclicVelocity { vel: f64 },
    /// Target forwarding: drive profiles internally toward `pos`.
    /// Only meaningful on `SetpointKind::TargetForwarding` axes.
    TargetPosition { pos: f64, vel: f64, acc: f64, dec: f64 },
    /// Target forwarding: sustained velocity with drive-internal ramps.
    TargetVelocity { vel: f64, acc: f64, dec: f64 },
    /// Run the backend/drive homing sequence (parameters are configured
    /// out-of-band via [`AcyclicAccess`] before requesting this).
    /// Completion is reported through `AxisIn::homed`.
    Home,
}

/// Per-axis outputs, motion-core → backend.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AxisOut {
    /// Desired power stage state. The adapter runs whatever handshake the
    /// drive needs (402 controlword sequencing) to converge on it.
    pub enable: bool,
    /// Level-held fault-reset request; the adapter edges it into the drive
    /// (402: controlword bit 7) and it is safe to hold across cycles.
    pub fault_reset: bool,
    pub setpoint: Setpoint,
}

/// Result of one cyclic exchange.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExchangeStatus {
    /// Every configured axis answered this cycle (EtherCAT: working counter
    /// matched; Modbus: the covering poll succeeded).
    pub all_axes_responding: bool,
    /// Inputs come from a transaction completed in *this* call. Soft-poll
    /// backends return `false` between polls (cached data).
    pub inputs_fresh: bool,
}

// ─── Bus state and events ────────────────────────────────────────────────────

/// Coarse bus lifecycle, EtherCAT vocabulary as the superset.
/// Modbus adapters map: disconnected → `Init`, connected → `Op`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BusState {
    Init,
    PreOp,
    SafeOp,
    Op,
}

/// Asynchronous bus happenings, drained from the cycle loop via
/// [`Fieldbus::poll_event`]. All variants are `Copy` — draining allocates
/// nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BusEvent {
    BusStateChanged(BusState),
    /// An axis (re)appeared and delivers valid inputs.
    AxisOnline(AxisId),
    /// An axis stopped responding (slave dropped, Modbus timeout).
    AxisOffline(AxisId),
    /// An axis reported a fault; `code` is backend-native.
    AxisFault { axis: AxisId, code: u32 },
}

// ─── Errors ──────────────────────────────────────────────────────────────────

/// Fieldbus error. `Copy` so the RT path can propagate it without allocating;
/// adapters log rich detail themselves before returning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldbusError {
    /// Operation not available on this backend / platform / axis.
    Unsupported(&'static str),
    /// Call not valid in the current bus state.
    InvalidState(BusState),
    /// The axis index is out of range for this bus.
    NoSuchAxis(AxisId),
    /// Cyclic exchange failed (timeout, WKC mismatch, socket error, …).
    Exchange(&'static str),
    /// Acyclic transfer rejected; `code` is backend-native (SDO abort code,
    /// Modbus exception).
    Acyclic { code: u32 },
    Timeout,
}

impl fmt::Display for FieldbusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(what) => write!(f, "unsupported: {what}"),
            Self::InvalidState(s) => write!(f, "invalid bus state: {s:?}"),
            Self::NoSuchAxis(a) => write!(f, "no such axis: {a}"),
            Self::Exchange(why) => write!(f, "cyclic exchange failed: {why}"),
            Self::Acyclic { code } => write!(f, "acyclic transfer failed, code {code:#x}"),
            Self::Timeout => write!(f, "timeout"),
        }
    }
}

impl std::error::Error for FieldbusError {}

// ─── Acyclic parameter channel ───────────────────────────────────────────────

/// Address of a device parameter, off the RT path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamAddr {
    /// CANopen object dictionary entry (CoE SDO; also fits CiA-402-over-Modbus
    /// register maps that mirror the object dictionary).
    Object { index: u16, sub: u8 },
    /// Raw register (Modbus holding register, vendor parameter number).
    Register { addr: u16 },
}

/// Non-cyclic parameter/diagnostic access (SDO, register read/write).
///
/// Handles are obtained via [`Fieldbus::acyclic`] and used from a *service*
/// thread, never from the cycle loop; adapters must make transfers safe to
/// run concurrently with the cyclic exchange (EtherCAT: mailbox traffic
/// interleaved with PDO).
#[allow(async_fn_in_trait)] // driven on the owning thread; no Send bound needed
pub trait AcyclicAccess {
    /// Read a parameter into `buf`; returns the number of bytes read.
    async fn read(
        &mut self,
        axis: AxisId,
        addr: ParamAddr,
        buf: &mut [u8],
    ) -> Result<usize, FieldbusError>;

    /// Write a parameter.
    async fn write(
        &mut self,
        axis: AxisId,
        addr: ParamAddr,
        data: &[u8],
    ) -> Result<(), FieldbusError>;
}

// ─── The bus itself ──────────────────────────────────────────────────────────

/// One bus instance (one EtherCAT MainDevice on one NIC, one Modbus
/// connection pool, one simulator).
///
/// Async by design: the decided RT model is a `smol` executor per cycle
/// thread with absolute-time pacing (`Timer::at`), and EtherCAT tx/rx is
/// inherently a future. Static dispatch only — the daemon monomorphises its
/// cycle loop per backend; no boxed futures on the hot path (see ADR-2).
#[allow(async_fn_in_trait)]
pub trait Fieldbus {
    /// Handle type for off-RT parameter access. Adapters define their own
    /// (this is where ethercrab's SDO plumbing hides).
    type Acyclic: AcyclicAccess;

    /// Number of axes this bus instance serves. Valid `AxisId`s are
    /// `0..axis_count()`. Stable after `start()`.
    fn axis_count(&self) -> usize;

    /// Capability of one axis. Stable after `start()`.
    fn capability(&self, axis: AxisId) -> AxisCapability;

    fn bus_state(&self) -> BusState;

    /// Cold → operational: scan the topology, configure (PDO mapping, DC,
    /// startup SDOs), and bring the bus to `Op`. Not RT; may allocate.
    async fn start(&mut self) -> Result<(), FieldbusError>;

    /// Graceful shutdown (drives disabled, bus back to `Init`).
    async fn stop(&mut self) -> Result<(), FieldbusError>;

    /// One cyclic exchange: publish `outs`, receive into `ins`.
    /// Both slices are exactly `axis_count()` long.
    ///
    /// Hard-RT backends: one PDO transaction, allocation-free, bounded time.
    /// Soft backends: may serve cached inputs (see [`CycleClass::SoftPoll`]).
    async fn exchange(
        &mut self,
        outs: &[AxisOut],
        ins: &mut [AxisIn],
    ) -> Result<ExchangeStatus, FieldbusError>;

    /// Drain one pending bus event; non-blocking, allocation-free.
    /// Call repeatedly until `None`.
    fn poll_event(&mut self) -> Option<BusEvent>;

    /// A fresh handle for acyclic parameter access, to be used off the RT
    /// thread. May be called multiple times.
    fn acyclic(&self) -> Self::Acyclic;
}
