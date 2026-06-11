METHOD JogNegative : structAxisControl_Result
VAR_INPUT
	lrVelocity     : LREAL := 0;	// 0 = 使用 stParameter.stDefault.lrVelocity
	lrAcceleration : LREAL := 0;	// 0 = 使用 stParameter.stDefault.lrAcc
	lrDeceleration : LREAL := 0;	// 0 = 使用 stParameter.stDefault.lrDec
	lrJerk         : LREAL := 0;	// 0 = 使用 stParameter.stDefault.lrJerk
END_VAR

// === IMPLEMENTATION ===
_lrJogVelocity := SEL(lrVelocity     = 0, lrVelocity,     stParameter.stDefault.lrVelocity);
_lrJogAcc      := SEL(lrAcceleration = 0, lrAcceleration, stParameter.stDefault.lrAcc);
_lrJogDec      := SEL(lrDeceleration = 0, lrDeceleration, stParameter.stDefault.lrDec);
_lrJogJerk     := SEL(lrJerk         = 0, lrJerk,         stParameter.stDefault.lrJerk);

_fbMC_MoveVelocity.Velocity     := _lrJogVelocity;
_fbMC_MoveVelocity.Acceleration := _lrJogAcc;
_fbMC_MoveVelocity.Deceleration := _lrJogDec;
_fbMC_MoveVelocity.Jerk         := _lrJogJerk;

// 設定請求旗標（每掃描週期持續呼叫 = 持續 Jog）
_xJogNegRequested := TRUE;

// 回傳當前狀態
JogNegative.xBusy    := (iStep = enumAxisControl_Step.JOG_NEG);
JogNegative.xDone    := FALSE;	// Jog 沒有完成概念
JogNegative.xError   := xError;
JogNegative.eErrorID := _eLastMotionError;