METHOD MoveRelative : structAxisControl_Result
VAR_INPUT
	lrDistance     : LREAL;		// 移動距離（正值=正方向，負值=負方向）
	lrVelocity     : LREAL := 0;	// 0 = 使用 stParameter.stDefault.lrVelocity
	lrAcceleration : LREAL := 0;	// 0 = 使用 stParameter.stDefault.lrAcc
	lrDeceleration : LREAL := 0;	// 0 = 使用 stParameter.stDefault.lrDec
	lrJerk         : LREAL := 0;	// 0 = 使用 stParameter.stDefault.lrJerk
END_VAR

// === IMPLEMENTATION ===
// 解析有效參數（0 = 使用預設）
_fbMC_MoveRelative.Distance     := lrDistance;
_fbMC_MoveRelative.Velocity     := SEL(lrVelocity     = 0, lrVelocity,     stParameter.stDefault.lrVelocity);
_fbMC_MoveRelative.Acceleration := SEL(lrAcceleration = 0, lrAcceleration, stParameter.stDefault.lrAcc);
_fbMC_MoveRelative.Deceleration := SEL(lrDeceleration = 0, lrDeceleration, stParameter.stDefault.lrDec);
_fbMC_MoveRelative.Jerk         := SEL(lrJerk         = 0, lrJerk,         stParameter.stDefault.lrJerk);

// 設定請求旗標
_xMoveRelRequested := TRUE;

// 回傳當前狀態
MoveRelative.xBusy    := (iStep = enumAxisControl_Step.MOVE_REL);
MoveRelative.xDone    := _xCommandDone AND (_iActiveCommand = enumAxisControl_Step.MOVE_REL);
MoveRelative.xError   := xError;
MoveRelative.eErrorID := _eLastMotionError;