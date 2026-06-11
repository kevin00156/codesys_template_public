METHOD StopMotion : structAxisControl_Result

// === IMPLEMENTATION ===
// 區分兩種 Stop 場景：
// - xEnable=FALSE：完全斷電，回到 IDLE（iStep=0）
// - xStop=TRUE：  停止運動，保持供電（狀態機繼續管理）
IF NOT xEnable THEN
	_xPowerOn := FALSE;
	_ClearAllExecute();
	_fbMC_Stop.Execute := FALSE;
	SUPER^.Stop();	// iStep := 0, xBusy/xDone/xError := FALSE
ELSE
	// xStop=TRUE 場景：只在不已處於停止流程時才轉換
	IF iStep <> enumAxisControl_Step.STOPPING AND
	   iStep <> enumAxisControl_Step.WAIT_STOP THEN
		_ClearAllExecute();
		iStep := enumAxisControl_Step.STOPPING;
	END_IF
END_IF

// 回傳當前狀態
StopMotion.xBusy := (iStep = enumAxisControl_Step.STOPPING OR iStep = enumAxisControl_Step.WAIT_STOP);
StopMotion.xDone := _xCommandDone AND (_iActiveCommand = enumAxisControl_Step.STOPPING);
StopMotion.xError := xError;
StopMotion.eErrorID := _eLastMotionError;