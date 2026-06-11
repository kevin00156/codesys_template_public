METHOD DoSetPosition : INT
VAR_INPUT
	lrPosition : LREAL;
END_VAR

// === IMPLEMENTATION ===
// X57s 不支援標準 MC_SetPosition，使用 Homing Mode 35 作為 workaround
// _xSetPosFirstCall 確保每次進入 SET_POSITION 狀態時有 Execute 上升沿

IF _xSetPosFirstCall THEN
	// 首次呼叫：送出 Reset 脈衝，確保 FB 從乾淨狀態開始
	_fbSetPosition_ActualDriver(Axis := Axis, Execute := FALSE, Position := lrPosition, xReset := TRUE);
	_xSetPosFirstCall := FALSE;
ELSE
	_fbSetPosition_ActualDriver(Axis := Axis, Execute := TRUE, Position := lrPosition, xReset := FALSE);
END_IF

IF _fbSetPosition_ActualDriver.Done THEN
	_xSetPosFirstCall := TRUE;		// 重置，供下次使用
	DoSetPosition := 1;
ELSIF _fbSetPosition_ActualDriver.Error THEN
	_xSetPosFirstCall := TRUE;
	DoSetPosition := -1;
ELSE
	DoSetPosition := 0;
END_IF