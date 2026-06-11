METHOD SetPosition : structAxisControl_Result
VAR_INPUT
	lrPosition : LREAL;		// 目標位置設定值
END_VAR

// === IMPLEMENTATION ===
_lrSetPosTarget   := lrPosition;
_xSetPosRequested := TRUE;

// 回傳當前狀態
SetPosition.xBusy    := (iStep = enumAxisControl_Step.SET_POSITION);
SetPosition.xDone    := _xCommandDone AND (_iActiveCommand = enumAxisControl_Step.SET_POSITION);
SetPosition.xError   := xError;
SetPosition.eErrorID := _eLastMotionError;