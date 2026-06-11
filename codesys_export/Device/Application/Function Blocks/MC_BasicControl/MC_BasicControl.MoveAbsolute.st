METHOD MoveAbsolute : structAxisControl_Result
VAR_INPUT
	lrPosition     : LREAL;		// 目標絕對位置（必填）
	lrVelocity     : LREAL := 0;	// 0 = 使用 stParameter.stDefault.lrVelocity
	lrAcceleration : LREAL := 0;	// 0 = 使用 stParameter.stDefault.lrAcc
	lrDeceleration : LREAL := 0;	// 0 = 使用 stParameter.stDefault.lrDec
	lrJerk         : LREAL := 0;	// 0 = 使用 stParameter.stDefault.lrJerk
END_VAR

// === IMPLEMENTATION ===
// 解析有效參數（0 = 使用預設）
_fbMC_MoveAbsolute.Position     := lrPosition;
_fbMC_MoveAbsolute.Velocity     := SEL(lrVelocity     = 0, lrVelocity,     stParameter.stDefault.lrVelocity);
_fbMC_MoveAbsolute.Acceleration := SEL(lrAcceleration = 0, lrAcceleration, stParameter.stDefault.lrAcc);
_fbMC_MoveAbsolute.Deceleration := SEL(lrDeceleration = 0, lrDeceleration, stParameter.stDefault.lrDec);
_fbMC_MoveAbsolute.Jerk         := SEL(lrJerk         = 0, lrJerk,         stParameter.stDefault.lrJerk);

// 設定請求旗標（下一掃描週期 FB body 讀取 _Prev 版本）
_xMoveAbsRequested := TRUE;

// 回傳當前狀態
MoveAbsolute.xBusy  := (iStep = enumAxisControl_Step.MOVE_ABS);
MoveAbsolute.xDone  := _xCommandDone AND (_iActiveCommand = enumAxisControl_Step.MOVE_ABS);
MoveAbsolute.xError := xError;
MoveAbsolute.eErrorID := _eLastMotionError;