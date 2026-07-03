//! `enumAxisControl_Step` — the HMI renders these numeric values, so they are
//! pinned to `codesys_export/.../Structure/enumAxisControl_Step.st` exactly.

/// Axis state machine step. Published in `AxisState.Step` as `i32`.
#[repr(i32)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Step {
    /// 閒置/關電狀態（BaseStateMachine Stop() 後的起點）
    #[default]
    Idle = 0,
    /// 等待驅動器激磁
    Enabling = 10,
    /// 就緒，等待命令
    Ready = 20,
    /// 軸狀態異常，等待恢復
    NotReady = 25,
    /// 正向 JOG 執行中
    JogPos = 30,
    /// 反向 JOG 執行中
    JogNeg = 31,
    /// 絕對定位執行中
    MoveAbs = 40,
    /// 相對定位執行中
    MoveRel = 50,
    /// 速度模式執行中
    MoveVel = 60,
    /// 寫入回原點參數中
    HomingWriteParam = 70,
    /// 執行回原點中
    HomingExec = 71,
    /// 設定位置執行中
    SetPosition = 80,
    /// MC_Stop 命令發出中
    Stopping = 90,
    /// 等待 MC_Stop 完成
    WaitStop = 91,
    /// 嘗試重置伺服錯誤
    TryReset = 95,
}

impl Step {
    pub fn as_i32(self) -> i32 {
        self as i32
    }

    /// Steps the EMS handler forces into `Stopping` (the list in
    /// `MC_BasicControl.Main.st`'s EMS block).
    pub fn is_motion(self) -> bool {
        matches!(
            self,
            Step::JogPos
                | Step::JogNeg
                | Step::MoveAbs
                | Step::MoveRel
                | Step::MoveVel
                | Step::HomingWriteParam
                | Step::HomingExec
                | Step::SetPosition
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Values pinned to enumAxisControl_Step.st — the HMI depends on them.
    #[test]
    fn step_values_match_iec_enum() {
        assert_eq!(Step::Idle.as_i32(), 0);
        assert_eq!(Step::Enabling.as_i32(), 10);
        assert_eq!(Step::Ready.as_i32(), 20);
        assert_eq!(Step::NotReady.as_i32(), 25);
        assert_eq!(Step::JogPos.as_i32(), 30);
        assert_eq!(Step::JogNeg.as_i32(), 31);
        assert_eq!(Step::MoveAbs.as_i32(), 40);
        assert_eq!(Step::MoveRel.as_i32(), 50);
        assert_eq!(Step::MoveVel.as_i32(), 60);
        assert_eq!(Step::HomingWriteParam.as_i32(), 70);
        assert_eq!(Step::HomingExec.as_i32(), 71);
        assert_eq!(Step::SetPosition.as_i32(), 80);
        assert_eq!(Step::Stopping.as_i32(), 90);
        assert_eq!(Step::WaitStop.as_i32(), 91);
        assert_eq!(Step::TryReset.as_i32(), 95);
    }
}
