//! Bitmask vocabulary of the shm contract — mirrors the comments in
//! `backend/internal/shm/layout.go` (which the HMI and Modbus map also use).

/// `AxisCmd.ControlFlags` bits (HMI → daemon).
pub mod cmd {
    pub const ENABLE: u32 = 1 << 0;
    pub const HOME: u32 = 1 << 1;
    pub const RESET: u32 = 1 << 2;
    pub const STOP: u32 = 1 << 3;
    pub const JOG_POS: u32 = 1 << 4;
    pub const JOG_NEG: u32 = 1 << 5;
    pub const MOVE_ABS: u32 = 1 << 6;
}

/// `AxisState.Flags` bits (daemon → HMI).
pub mod status {
    pub const ENABLED: u32 = 1 << 0;
    pub const BUSY: u32 = 1 << 1;
    pub const ERROR: u32 = 1 << 2;
    pub const STANDSTILL: u32 = 1 << 3;
    pub const POS_LIMIT: u32 = 1 << 4;
    pub const NEG_LIMIT: u32 = 1 << 5;
}

/// `MachineCmd.ControlFlags` bits (HMI → daemon).
pub mod machine_cmd {
    pub const RESET: u32 = 1 << 0;
    pub const EMS: u32 = 1 << 1;
    pub const SYSTEM_RUN: u32 = 1 << 2;
}

/// `enumAxisControl_ErrorID` values, pinned to
/// `codesys_export/.../Structure/enumAxisControl_ErrorID.st`.
pub mod error_id {
    pub const NONE: i32 = 0;
    /// 驅動器回報錯誤（fault code 另見 AxisIn::fault_code）
    pub const DRIVER_ERROR: i32 = 100;
    pub const HOMING_WRITE_PARAM_FAIL: i32 = 201;
    pub const HOMING_EXEC_FAIL: i32 = 202;
    pub const MOVE_ABS_FAIL: i32 = 301;
    pub const MOVE_REL_FAIL: i32 = 302;
    pub const MOVE_VEL_FAIL: i32 = 303;
    pub const SET_POSITION_FAIL: i32 = 401;
    pub const POSITIVE_LIMIT_TRIP: i32 = 501;
    pub const NEGATIVE_LIMIT_TRIP: i32 = 502;
}
