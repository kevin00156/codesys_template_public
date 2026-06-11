METHOD MoveVelocity : structAxisControl_Result
VAR_INPUT
	lrVelocity     : LREAL := 0;	// 速度（正=正向，負=負向），0 使用預設
	lrAcceleration : LREAL := 0;	// 0 = 使用 stParameter.stDefault.lrAcc
	lrDeceleration : LREAL := 0;	// 0 = 使用 stParameter.stDefault.lrDec
	lrJerk         : LREAL := 0;	// 0 = 使用 stParameter.stDefault.lrJerk
END_VAR

// === IMPLEMENTATION ===
_fbMC_MoveVelocity.Velocity     := SEL(lrVelocity     = 0, lrVelocity,     stParameter.stDefault.lrVelocity);
_fbMC_MoveVelocity.Acceleration := SEL(lrAcceleration = 0, lrAcceleration, stParameter.stDefault.lrAcc);
_fbMC_MoveVelocity.Deceleration := SEL(lrDeceleration = 0, lrDeceleration, stParameter.stDefault.lrDec);
_fbMC_MoveVelocity.Jerk         := SEL(lrJerk         = 0, lrJerk,         stParameter.stDefault.lrJerk);

// 設定請求旗標
_xMoveVelRequested := TRUE;

// 回傳當前狀態（MoveVelocity 沒有 xDone，只有 xBusy = InVelocity）
MoveVelocity.xBusy    := (iStep = enumAxisControl_Step.MOVE_VEL);
MoveVelocity.xDone    := FALSE;
MoveVelocity.xError   := xError;
MoveVelocity.eErrorID := _eLastMotionError;