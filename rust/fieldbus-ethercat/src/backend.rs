//! The real ethercrab adapter (Linux only — raw sockets).
//!
//! Lifecycle: `start()` = topology scan → PREOP configuration (CSP mode,
//! PDO mapping, optional DC SYNC0) → OP, with the tx/rx driver on its own
//! thread. `exchange()` = one LRW cycle: write controlword/targets into the
//! PDI, transact, decode statuswords/actuals. All per-cycle state lives in
//! preallocated fixed-size buffers — the hot path performs no heap
//! allocation and holds the PDI spinlock guard only for the few-byte copies.
//!
//! ethercrab types stay `pub(crate)` at most; the public surface is
//! `EthercatBackend` + `EcatConfig` only.

use std::time::Duration;

use ethercrab::{
    std::{ethercat_now, tx_rx_task},
    subdevice_group::{CycleInfo, DcConfiguration, HasDc, NoDc, Op, PreOp},
    DcSync, MainDevice, MainDeviceConfig, PduStorage, SubDeviceGroup, SubDeviceState, Timeouts,
};
use fieldbus_api::{
    AcyclicAccess, AxisCapability, AxisId, AxisIn, AxisOut, BusEvent, BusState, CycleClass,
    DriveStatus, ExchangeStatus, Fieldbus, FieldbusError, OpModes, ParamAddr, Setpoint,
    SetpointKind,
};

use crate::cia402::Cia402;
use crate::config::EcatConfig;
use crate::pdo;

const MAX_FRAMES: usize = 16;
const MAX_PDU_DATA: usize = PduStorage::element_size(1100);
const MAX_SUBDEVICES: usize = 16;
const MAX_PDI: usize = 128;

type Storage = PduStorage<MAX_FRAMES, MAX_PDU_DATA>;
type PreOpGroup = SubDeviceGroup<MAX_SUBDEVICES, MAX_PDI, PreOp>;

/// The group after `request_into_op`, with or without distributed clocks —
/// decided by config at runtime, so both variants live behind one enum.
enum OpGroup {
    NoDc(SubDeviceGroup<MAX_SUBDEVICES, MAX_PDI, Op, NoDc>),
    Dc(SubDeviceGroup<MAX_SUBDEVICES, MAX_PDI, Op, HasDc>),
}

/// Per-axis runtime state (fixed-size, allocated once at start).
struct AxisRt {
    subdevice: usize,
    scale: f64,
    seq: Cia402,
    /// Statusword from the previous cycle's inputs — the controlword always
    /// reacts one cycle late, which the 402 handshake tolerates by design.
    statusword: u16,
    /// Last target written, in increments. While the drive is not enabled
    /// this tracks actual position so CSP never sees a jump on enable.
    target: i32,
    enable: bool,
    was_fault: bool,
}

struct Running {
    maindevice: MainDevice<'static>,
    group: OpGroup,
}

/// Fixed-capacity event ring: the cycle path pushes without allocating;
/// overflow drops the newest event (events are advisory diagnostics).
struct EventRing {
    buf: [Option<BusEvent>; 64],
    head: usize,
    len: usize,
}

impl EventRing {
    fn new() -> Self {
        EventRing {
            buf: [None; 64],
            head: 0,
            len: 0,
        }
    }

    fn push(&mut self, ev: BusEvent) {
        if self.len == self.buf.len() {
            return;
        }
        self.buf[(self.head + self.len) % self.buf.len()] = Some(ev);
        self.len += 1;
    }

    fn pop(&mut self) -> Option<BusEvent> {
        if self.len == 0 {
            return None;
        }
        let ev = self.buf[self.head].take();
        self.head = (self.head + 1) % self.buf.len();
        self.len -= 1;
        ev
    }
}

pub struct EthercatBackend {
    cfg: EcatConfig,
    state: BusState,
    axes: Vec<AxisRt>,
    events: EventRing,
    running: Option<Running>,
    /// Whole-group responsiveness from the previous cycle (edge detection
    /// for AxisOffline/AxisOnline events).
    was_responding: bool,
}

impl EthercatBackend {
    pub fn new(cfg: EcatConfig) -> EthercatBackend {
        let axes = cfg
            .axes
            .iter()
            .map(|m| AxisRt {
                subdevice: m.subdevice,
                scale: m.scale,
                seq: Cia402::default(),
                statusword: 0,
                target: 0,
                enable: false,
                was_fault: false,
            })
            .collect();
        EthercatBackend {
            cfg,
            state: BusState::Init,
            axes,
            events: EventRing::new(),
            running: None,
            was_responding: false,
        }
    }

    /// PREOP: put every configured drive into CSP and (optionally) write our
    /// PDO mapping. Runs before the PDI exists, so plain SDO traffic.
    async fn configure_drives(
        &self,
        maindevice: &MainDevice<'static>,
        group: &PreOpGroup,
    ) -> Result<(), FieldbusError> {
        for m in &self.cfg.axes {
            let sd = group
                .subdevice(maindevice, m.subdevice)
                .map_err(|_| FieldbusError::NoSuchAxis(AxisId(m.subdevice)))?;

            sd.sdo_write(pdo::obj::MODES_OF_OPERATION, 0, pdo::obj::MODE_CSP)
                .await
                .map_err(|e| {
                    eprintln!("ecat: subdevice {}: set CSP mode: {e}", m.subdevice);
                    FieldbusError::Exchange("set modes of operation")
                })?;

            if !self.cfg.configure_pdo {
                continue;
            }

            let map_err = |what: &'static str| {
                move |e: ethercrab::error::Error| {
                    eprintln!("ecat: subdevice {}: {what}: {e}", m.subdevice);
                    FieldbusError::Exchange(what)
                }
            };

            // RxPDO: 0x1C12 → 0x1600 → our mapping.
            sd.sdo_write(pdo::obj::SM2_PDO_ASSIGN, 0, 0u8)
                .await
                .map_err(map_err("clear 0x1C12"))?;
            sd.sdo_write(pdo::obj::RX_PDO_MAP, 0, 0u8)
                .await
                .map_err(map_err("clear 0x1600"))?;
            for (i, entry) in pdo::RX_MAPPING.iter().enumerate() {
                sd.sdo_write(pdo::obj::RX_PDO_MAP, (i + 1) as u8, *entry)
                    .await
                    .map_err(map_err("write 0x1600 entry"))?;
            }
            sd.sdo_write(pdo::obj::RX_PDO_MAP, 0, pdo::RX_MAPPING.len() as u8)
                .await
                .map_err(map_err("finalise 0x1600"))?;
            sd.sdo_write(pdo::obj::SM2_PDO_ASSIGN, 1, pdo::obj::RX_PDO_MAP)
                .await
                .map_err(map_err("assign 0x1600"))?;
            sd.sdo_write(pdo::obj::SM2_PDO_ASSIGN, 0, 1u8)
                .await
                .map_err(map_err("finalise 0x1C12"))?;

            // TxPDO: 0x1C13 → 0x1A00 → our mapping (+ optional 0x60FD).
            sd.sdo_write(pdo::obj::SM3_PDO_ASSIGN, 0, 0u8)
                .await
                .map_err(map_err("clear 0x1C13"))?;
            sd.sdo_write(pdo::obj::TX_PDO_MAP, 0, 0u8)
                .await
                .map_err(map_err("clear 0x1A00"))?;
            let mut n = 0u8;
            for entry in pdo::TX_MAPPING_BASE {
                n += 1;
                sd.sdo_write(pdo::obj::TX_PDO_MAP, n, *entry)
                    .await
                    .map_err(map_err("write 0x1A00 entry"))?;
            }
            if self.cfg.map_digital_inputs {
                n += 1;
                sd.sdo_write(pdo::obj::TX_PDO_MAP, n, pdo::TX_MAPPING_DIN)
                    .await
                    .map_err(map_err("write 0x1A00 din entry"))?;
            }
            sd.sdo_write(pdo::obj::TX_PDO_MAP, 0, n)
                .await
                .map_err(map_err("finalise 0x1A00"))?;
            sd.sdo_write(pdo::obj::SM3_PDO_ASSIGN, 1, pdo::obj::TX_PDO_MAP)
                .await
                .map_err(map_err("assign 0x1A00"))?;
            sd.sdo_write(pdo::obj::SM3_PDO_ASSIGN, 0, 1u8)
                .await
                .map_err(map_err("finalise 0x1C13"))?;
        }
        Ok(())
    }

    /// One PDI pass for one axis: encode outputs from the *previous* cycle's
    /// statusword, then (after tx_rx, second closure) decode fresh inputs.
    fn write_axis_outputs(group_sd_out: &mut [u8], axis: &mut AxisRt, out: &AxisOut) {
        let enabled = Cia402::drive_status(axis.enable, axis.statusword) == DriveStatus::Enabled;
        axis.enable = out.enable;

        let cw = axis.seq.controlword(out.enable, out.fault_reset, axis.statusword);
        let target = if enabled {
            match out.setpoint {
                Setpoint::CyclicPosition { pos, .. } => (pos * axis.scale).round() as i32,
                // Hold and everything CSP can't express: freeze the last target.
                _ => axis.target,
            }
        } else {
            // Not enabled: track actual so enabling never commands a jump.
            axis.target
        };
        axis.target = target;
        pdo::encode_rx(&mut group_sd_out[..pdo::RX_BYTES], cw, target);
    }

    fn read_axis_inputs(&mut self, i: usize, raw: &[u8], responding: bool, ins: &mut AxisIn) {
        let with_din = self.cfg.map_digital_inputs;
        let axis = &mut self.axes[i];
        let tx = pdo::decode_tx(&raw[..pdo::tx_bytes(with_din)], with_din);
        axis.statusword = tx.statusword;

        let drive = if responding {
            Cia402::drive_status(axis.enable, tx.statusword)
        } else {
            DriveStatus::Offline
        };

        // While the drive is not following targets, mirror actual → target
        // (CSP anti-jump; also the initial value right after OP).
        if drive != DriveStatus::Enabled {
            axis.target = tx.position;
        }

        let fault = drive == DriveStatus::Fault;
        if fault && !axis.was_fault {
            self.events.push(BusEvent::AxisFault {
                axis: AxisId(i),
                code: tx.statusword as u32,
            });
        }
        axis.was_fault = fault;

        *ins = AxisIn {
            act_pos: tx.position as f64 / axis.scale,
            act_vel: tx.velocity as f64 / axis.scale,
            drive,
            fault_code: 0, // 0x603F needs mailbox traffic; deferred with homing work
            pos_limit: tx.digital_inputs & pdo::DIN_POS_LIMIT != 0,
            neg_limit: tx.digital_inputs & pdo::DIN_NEG_LIMIT != 0,
            homed: false, // homing mode lands with the drive domain knowledge
        };
    }
}

impl Fieldbus for EthercatBackend {
    type Acyclic = EthercatAcyclic;

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
                velocity: false, // CSV mode: later, if a machine needs it
                torque: false,
                homing: false, // 402 homing mode lands with MC_WriteHomingParameters port
            },
        }
    }

    fn bus_state(&self) -> BusState {
        self.state
    }

    async fn start(&mut self) -> Result<(), FieldbusError> {
        if self.running.is_some() {
            return Err(FieldbusError::InvalidState(self.state));
        }

        // One leaked storage per bus instance — the daemon builds at most a
        // handful of backends per process lifetime, and `PduStorage` must
        // outlive the detached tx/rx thread anyway. Dual master = two
        // backends = two storages on two NICs; nothing here is global.
        let storage: &'static Storage = Box::leak(Box::new(Storage::new()));
        let (tx, rx, pdu_loop) = storage
            .try_split()
            .map_err(|_| FieldbusError::Exchange("PduStorage already split"))?;

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

        let txrx = tx_rx_task(&self.cfg.ifname, tx, rx).map_err(|e| {
            eprintln!("ecat: open {}: {e}", self.cfg.ifname);
            FieldbusError::Exchange("open network interface")
        })?;
        std::thread::Builder::new()
            .name("ecat-txrx".into())
            .spawn(move || {
                if let Err(e) = smol::block_on(txrx) {
                    eprintln!("ecat: tx/rx task exited: {e}");
                }
            })
            .map_err(|_| FieldbusError::Exchange("spawn tx/rx thread"))?;

        // ── scan ────────────────────────────────────────────────────────
        let mut group: PreOpGroup = maindevice
            .init_single_group::<MAX_SUBDEVICES, MAX_PDI>(ethercat_now)
            .await
            .map_err(|e| {
                eprintln!("ecat: init/scan: {e}");
                FieldbusError::Exchange("topology scan")
            })?;
        self.state = BusState::PreOp;
        self.events.push(BusEvent::BusStateChanged(BusState::PreOp));

        eprintln!("ecat: {} subdevice(s) on {}", group.len(), self.cfg.ifname);
        for sd in group.iter(&maindevice) {
            eprintln!(
                "ecat:   {:#06x} {:?} {}",
                sd.configured_address(),
                sd.identity(),
                sd.name(),
            );
        }
        for m in &self.cfg.axes {
            if m.subdevice >= group.len() {
                eprintln!(
                    "ecat: axis maps to subdevice {} but only {} present",
                    m.subdevice,
                    group.len()
                );
                return Err(FieldbusError::NoSuchAxis(AxisId(m.subdevice)));
            }
        }

        // ── PREOP configuration ─────────────────────────────────────────
        self.configure_drives(&maindevice, &group).await?;

        if self.cfg.dc {
            for (i, mut sd) in group.iter_mut(&maindevice).enumerate() {
                if self.cfg.axes.iter().any(|m| m.subdevice == i) {
                    sd.set_dc_sync(DcSync::Sync0);
                }
            }
        }

        // ── PDI + OP ────────────────────────────────────────────────────
        // request_into_op (not into_op): we must run the process data loop
        // while subdevices transition, to feed watchdogs and valid data.
        let mut group = if self.cfg.dc {
            let group = group
                .configure_dc_sync(
                    &maindevice,
                    DcConfiguration {
                        start_delay: Duration::from_millis(100),
                        sync0_period: self.cfg.cycle,
                        sync0_shift: self.cfg.cycle / 4,
                    },
                )
                .await
                .map_err(|e| {
                    eprintln!("ecat: DC config: {e}");
                    FieldbusError::Exchange("configure distributed clocks")
                })?;
            OpGroup::Dc(
                group
                    .request_into_op(&maindevice)
                    .await
                    .map_err(|e| {
                        eprintln!("ecat: request OP: {e}");
                        FieldbusError::Exchange("request OP")
                    })?,
            )
        } else {
            let group = group.into_pre_op_pdi(&maindevice).await.map_err(|e| {
                eprintln!("ecat: FMMU config: {e}");
                FieldbusError::Exchange("configure PDI")
            })?;
            OpGroup::NoDc(
                group
                    .request_into_op(&maindevice)
                    .await
                    .map_err(|e| {
                        eprintln!("ecat: request OP: {e}");
                        FieldbusError::Exchange("request OP")
                    })?,
            )
        };
        self.state = BusState::SafeOp;
        self.events.push(BusEvent::BusStateChanged(BusState::SafeOp));

        // Drive the cycle until every subdevice reports OP (bounded wait).
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let all_op = match &mut group {
                OpGroup::NoDc(g) => {
                    let r = g.tx_rx(&maindevice).await.map_err(|e| {
                        eprintln!("ecat: tx/rx while entering OP: {e}");
                        FieldbusError::Exchange("tx/rx while entering OP")
                    })?;
                    r.is_in_state(SubDeviceState::Op)
                }
                OpGroup::Dc(g) => {
                    let r = g.tx_rx_dc(&maindevice).await.map_err(|e| {
                        eprintln!("ecat: tx/rx while entering OP: {e}");
                        FieldbusError::Exchange("tx/rx while entering OP")
                    })?;
                    r.is_in_state(SubDeviceState::Op)
                }
            };
            if all_op {
                break;
            }
            if std::time::Instant::now() > deadline {
                return Err(FieldbusError::Timeout);
            }
            smol::Timer::after(self.cfg.cycle).await;
        }

        // Validate that the drives' PDI regions match our expected layout —
        // fail loudly now rather than misdecode silently every cycle.
        for m in &self.cfg.axes {
            let (in_len, out_len) = match &group {
                OpGroup::NoDc(g) => {
                    let sd = g
                        .subdevice(&maindevice, m.subdevice)
                        .map_err(|_| FieldbusError::NoSuchAxis(AxisId(m.subdevice)))?;
                    let io = sd.io_raw();
                    (io.inputs().len(), io.outputs().len())
                }
                OpGroup::Dc(g) => {
                    let sd = g
                        .subdevice(&maindevice, m.subdevice)
                        .map_err(|_| FieldbusError::NoSuchAxis(AxisId(m.subdevice)))?;
                    let io = sd.io_raw();
                    (io.inputs().len(), io.outputs().len())
                }
            };
            let want_in = pdo::tx_bytes(self.cfg.map_digital_inputs);
            if in_len < want_in || out_len < pdo::RX_BYTES {
                eprintln!(
                    "ecat: subdevice {}: PDI {}i/{}o, expected ≥{}i/{}o — PDO mapping mismatch",
                    m.subdevice,
                    in_len,
                    out_len,
                    want_in,
                    pdo::RX_BYTES
                );
                return Err(FieldbusError::Exchange("PDI layout mismatch"));
            }
        }

        self.state = BusState::Op;
        self.events.push(BusEvent::BusStateChanged(BusState::Op));
        for i in 0..self.axes.len() {
            self.events.push(BusEvent::AxisOnline(AxisId(i)));
        }
        self.was_responding = true;
        self.running = Some(Running { maindevice, group });
        eprintln!("ecat: OP reached, cyclic exchange live");
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), FieldbusError> {
        let Some(mut running) = self.running.take() else {
            return Ok(());
        };
        // Best effort: a few cycles of controlword 0 (disable voltage) so
        // drives drop out of Operation Enabled before we abandon the bus.
        for _ in 0..5 {
            for axis in &mut self.axes {
                let write = |raw: &mut [u8]| {
                    pdo::encode_rx(&mut raw[..pdo::RX_BYTES], 0, axis.target);
                };
                match &running.group {
                    OpGroup::NoDc(g) => {
                        if let Ok(sd) = g.subdevice(&running.maindevice, axis.subdevice) {
                            write(&mut sd.io_raw_mut().outputs());
                        }
                    }
                    OpGroup::Dc(g) => {
                        if let Ok(sd) = g.subdevice(&running.maindevice, axis.subdevice) {
                            write(&mut sd.io_raw_mut().outputs());
                        }
                    }
                }
            }
            let _ = match &mut running.group {
                OpGroup::NoDc(g) => g.tx_rx(&running.maindevice).await.map(|_| ()),
                OpGroup::Dc(g) => g.tx_rx_dc(&running.maindevice).await.map(|_| ()),
            };
            smol::Timer::after(self.cfg.cycle).await;
        }
        self.state = BusState::Init;
        self.events.push(BusEvent::BusStateChanged(BusState::Init));
        Ok(())
    }

    async fn exchange(
        &mut self,
        outs: &[AxisOut],
        ins: &mut [AxisIn],
    ) -> Result<ExchangeStatus, FieldbusError> {
        if self.running.is_none() {
            return Err(FieldbusError::InvalidState(self.state));
        }
        debug_assert_eq!(outs.len(), self.axes.len());

        // 1. write outputs (controlword from last cycle's statusword)
        {
            let running = self.running.as_ref().unwrap();
            for (i, out) in outs.iter().enumerate() {
                let axis = &mut self.axes[i];
                match &running.group {
                    OpGroup::NoDc(g) => {
                        if let Ok(sd) = g.subdevice(&running.maindevice, axis.subdevice) {
                            let mut io = sd.io_raw_mut();
                            Self::write_axis_outputs(&mut io.outputs(), axis, out);
                        }
                    }
                    OpGroup::Dc(g) => {
                        if let Ok(sd) = g.subdevice(&running.maindevice, axis.subdevice) {
                            let mut io = sd.io_raw_mut();
                            Self::write_axis_outputs(&mut io.outputs(), axis, out);
                        }
                    }
                }
            }
        }

        // 2. one bus transaction
        let responding = {
            let running = self.running.as_mut().unwrap();
            match &mut running.group {
                OpGroup::NoDc(g) => {
                    let r = g
                        .tx_rx(&running.maindevice)
                        .await
                        .map_err(|_| FieldbusError::Exchange("tx/rx"))?;
                    r.is_in_state(SubDeviceState::Op)
                }
                OpGroup::Dc(g) => {
                    let r = g
                        .tx_rx_dc(&running.maindevice)
                        .await
                        .map_err(|_| FieldbusError::Exchange("tx/rx"))?;
                    let _cycle: &CycleInfo = &r.extra; // DC info available for diagnostics
                    r.is_in_state(SubDeviceState::Op)
                }
            }
        };

        if responding != self.was_responding {
            for i in 0..self.axes.len() {
                self.events.push(if responding {
                    BusEvent::AxisOnline(AxisId(i))
                } else {
                    BusEvent::AxisOffline(AxisId(i))
                });
            }
            self.was_responding = responding;
        }

        // 3. read fresh inputs
        for i in 0..self.axes.len() {
            let mut raw = [0u8; pdo::TX_BYTES_DIN];
            let n = pdo::tx_bytes(self.cfg.map_digital_inputs);
            {
                let running = self.running.as_ref().unwrap();
                let sub = self.axes[i].subdevice;
                match &running.group {
                    OpGroup::NoDc(g) => {
                        if let Ok(sd) = g.subdevice(&running.maindevice, sub) {
                            raw[..n].copy_from_slice(&sd.io_raw().inputs()[..n]);
                        }
                    }
                    OpGroup::Dc(g) => {
                        if let Ok(sd) = g.subdevice(&running.maindevice, sub) {
                            raw[..n].copy_from_slice(&sd.io_raw().inputs()[..n]);
                        }
                    }
                }
            }
            self.read_axis_inputs(i, &raw, responding, &mut ins[i]);
        }

        Ok(ExchangeStatus {
            all_axes_responding: responding,
            inputs_fresh: true,
        })
    }

    fn poll_event(&mut self) -> Option<BusEvent> {
        self.events.pop()
    }

    fn acyclic(&self) -> EthercatAcyclic {
        EthercatAcyclic {}
    }
}

/// SDO access handle. Not yet implemented: ethercrab's SDO plumbing is only
/// reachable through the group (SubDeviceRef::new is crate-private), and
/// mailbox traffic must not run on the cycle path. Lands together with the
/// homing work as a request queue serviced off-cycle — see README ADR-12.
pub struct EthercatAcyclic {}

impl AcyclicAccess for EthercatAcyclic {
    async fn read(
        &mut self,
        _axis: AxisId,
        _addr: ParamAddr,
        _buf: &mut [u8],
    ) -> Result<usize, FieldbusError> {
        Err(FieldbusError::Unsupported(
            "EtherCAT acyclic channel lands with the homing work (README ADR-12)",
        ))
    }

    async fn write(
        &mut self,
        _axis: AxisId,
        _addr: ParamAddr,
        _data: &[u8],
    ) -> Result<(), FieldbusError> {
        Err(FieldbusError::Unsupported(
            "EtherCAT acyclic channel lands with the homing work (README ADR-12)",
        ))
    }
}
