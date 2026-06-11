FUNCTION_BLOCK MC_BasicControl_LC10E EXTENDS MC_BasicControl
VAR_INPUT
	wDriverInputWord : WORD;	// X57s PDO 輸入字（位元映射見 ReadDriverInputs）
	wDriverErrorCode : WORD;	// X57s 錯誤碼（PDO 讀取）
END_VAR
VAR
	_fbSetPosition_ActualDriver : MC_ActualDriver_SetPosition;	// Homing Mode 35 workaround
	_xSetPosFirstCall   : BOOL := TRUE;			// 確保 Execute 有上升沿
END_VAR

// === IMPLEMENTATION ===
SUPER^(Axis := Axis, stParameter := stParameter);