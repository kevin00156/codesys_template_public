//! Byte-for-byte mirror of `backend/internal/shm/layout.go`, which is itself
//! a mirror of the IEC DUTs in `codesys_export/Device/Application/DUT/ShmBridge/`.
//!
//! The Go file is the single source of truth. Every layout change MUST bump
//! the matching version constant on all sides; readers refuse to mount a
//! segment with an unrecognised version. New fields are appended at the end,
//! existing offsets never move.
//!
//! Go on amd64 and IEC `pack_mode 8` both use natural 8-byte alignment, and
//! so does Rust `#[repr(C)]` here; padding is nevertheless spelled out as
//! explicit `_pad` fields so the layout is visible and the structs have no
//! hidden uninitialised bytes. Sizes and offsets are pinned by compile-time
//! asserts below (the counterpart of `vet.go`), and cross-language equality
//! is proven against shared golden fixtures in `tests/go_parity.rs`.

pub const PLC_DATA_MAGIC: u32 = 0x504C_4344; // 'PLCD'
pub const PLC_COMMAND_MAGIC: u32 = 0x504C_4343; // 'PLCC'

pub const PLC_DATA_VERSION: u16 = 4;
pub const PLC_COMMAND_VERSION: u16 = 3;

pub const NAME_PLC_DATA: &str = "plc_data";
pub const NAME_PLC_COMMAND: &str = "plc_cmd";

/// First 24 bytes of every segment.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Header {
    pub magic: u32,
    pub version: u16,
    pub flags: u16,
    pub seq: u32,
    pub _pad: u32,
    pub cycle: u64,
}

// ─── System ──────────────────────────────────────────────────────────────────

/// Published by the PLC/daemon each cycle (`PlcData.System`). 16 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SystemState {
    /// System temperature °C.
    pub temperature: f64,
    /// Bitmask — project-defined.
    pub status_flags: u32,
    /// Bitmask — project-defined.
    pub alarm_flags: u32,
}

// ─── Machine ─────────────────────────────────────────────────────────────────

/// Mirrors the key fields from `structMC_BasicControl_VisuStatus`.
/// 48 bytes per axis.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AxisState {
    /// Actual position (mm / user unit).
    pub act_pos: f64,
    /// Actual velocity (mm/s).
    pub act_vel: f64,
    /// Command position (mm / user unit).
    pub set_pos: f64,
    /// Command velocity (mm/s).
    pub set_vel: f64,
    /// `enumAxisControl_Step` value.
    pub step: i32,
    /// bit0=Enabled bit1=Busy bit2=Error bit3=StandStill bit4=PosLimit bit5=NegLimit.
    pub flags: u32,
    /// SMC_ERROR / axis error code.
    pub error_id: i32,
    pub _pad: i32,
}

/// Status of all axes plus machine-level flags. 200 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MachineState {
    pub axes: [AxisState; 4],
    /// Machine run state — project-defined enum.
    pub run_state: u32,
    /// Machine alarm bitmask.
    pub alarms: u32,
}

/// HMI commands for one axis (`PlcCommand.Machine.Axes[i]`). 32 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AxisCmd {
    /// bit0=Enable bit1=Home bit2=Reset bit3=Stop bit4=JogPos bit5=JogNeg bit6=MoveAbs.
    pub control_flags: u32,
    pub _pad: u32,
    pub jog_vel: f64,
    pub move_abs_pos: f64,
    pub move_abs_vel: f64,
}

/// HMI commands for the whole machine. 136 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MachineCmd {
    pub axes: [AxisCmd; 4],
    /// bit0=Reset bit1=EMS bit2=SystemRun — project-defined.
    pub control_flags: u32,
    pub _pad: u32,
}

// ─── Production ──────────────────────────────────────────────────────────────

/// Bidirectional: PLC/daemon publishes it, HMI can request changes. 8 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ProductionState {
    pub n_production_state: i32,
    pub _pad: i32,
}

// ─── Segment structs ─────────────────────────────────────────────────────────

/// Written by the PLC/daemon, read by Go. 248 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PlcData {
    pub header: Header,          // offset 0
    pub system: SystemState,     // offset 24
    pub machine: MachineState,   // offset 40
    pub production: ProductionState, // offset 240
}

/// Written by Go, read by the PLC/daemon. 168 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PlcCommand {
    pub header: Header,          // offset 0
    pub machine: MachineCmd,     // offset 24
    pub production: ProductionState, // offset 160
}

pub const SIZE_PLC_DATA: usize = core::mem::size_of::<PlcData>();
pub const SIZE_PLC_COMMAND: usize = core::mem::size_of::<PlcCommand>();

/// Marker for the two shm segment types.
///
/// # Safety
/// Implementors must be `#[repr(C)]`, start with [`Header`] at offset 0,
/// contain only integer/float fields (any bit pattern is a valid value, no
/// pointers, no implicit padding — all padding is explicit `_pad` fields),
/// and have a size that is a multiple of 8.
pub unsafe trait Segment: Copy + Default + 'static {
    const MAGIC: u32;
    const VERSION: u16;
    const NAME: &'static str;

    fn header(&self) -> &Header;
    fn header_mut(&mut self) -> &mut Header;
}

unsafe impl Segment for PlcData {
    const MAGIC: u32 = PLC_DATA_MAGIC;
    const VERSION: u16 = PLC_DATA_VERSION;
    const NAME: &'static str = NAME_PLC_DATA;

    fn header(&self) -> &Header {
        &self.header
    }
    fn header_mut(&mut self) -> &mut Header {
        &mut self.header
    }
}

unsafe impl Segment for PlcCommand {
    const MAGIC: u32 = PLC_COMMAND_MAGIC;
    const VERSION: u16 = PLC_COMMAND_VERSION;
    const NAME: &'static str = NAME_PLC_COMMAND;

    fn header(&self) -> &Header {
        &self.header
    }
    fn header_mut(&mut self) -> &mut Header {
        &mut self.header
    }
}

/// View a segment struct as raw bytes (for golden tests and tools).
pub fn as_bytes<T: Segment>(v: &T) -> &[u8] {
    // Safety: Segment guarantees no implicit padding, so every byte is
    // initialised; the lifetime is tied to the borrow.
    unsafe { core::slice::from_raw_parts(v as *const T as *const u8, core::mem::size_of::<T>()) }
}

/// Decode a segment struct from raw bytes (length must match exactly).
pub fn from_bytes<T: Segment>(b: &[u8]) -> T {
    assert_eq!(b.len(), core::mem::size_of::<T>(), "fixture size mismatch");
    // Safety: Segment guarantees any bit pattern is valid; read_unaligned
    // tolerates arbitrary source alignment.
    unsafe { (b.as_ptr() as *const T).read_unaligned() }
}

// ─── Compile-time size/offset pins (counterpart of vet.go) ───────────────────

use core::mem::{offset_of, size_of};

const _: () = assert!(size_of::<Header>() == 24);
const _: () = assert!(size_of::<SystemState>() == 16);
const _: () = assert!(size_of::<AxisState>() == 48);
const _: () = assert!(size_of::<AxisCmd>() == 32);
const _: () = assert!(size_of::<MachineState>() == 200);
const _: () = assert!(size_of::<MachineCmd>() == 136);
const _: () = assert!(size_of::<ProductionState>() == 8);
const _: () = assert!(size_of::<PlcData>() == 248);
const _: () = assert!(size_of::<PlcCommand>() == 168);

// The seqlock accesses the header through raw offsets; pin them.
const _: () = assert!(offset_of!(Header, seq) == 8);
const _: () = assert!(offset_of!(Header, cycle) == 16);

const _: () = assert!(offset_of!(PlcData, system) == 24);
const _: () = assert!(offset_of!(PlcData, machine) == 40);
const _: () = assert!(offset_of!(PlcData, production) == 240);
const _: () = assert!(offset_of!(PlcCommand, machine) == 24);
const _: () = assert!(offset_of!(PlcCommand, production) == 160);

// Word-granular seqlock copy requires 8-byte-multiple sizes.
const _: () = assert!(size_of::<PlcData>() % 8 == 0);
const _: () = assert!(size_of::<PlcCommand>() % 8 == 0);
