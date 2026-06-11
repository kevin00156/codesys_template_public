METHOD PRIVATE _RunMotionFBs

// === IMPLEMENTATION ===
// 每掃描週期呼叫主要 SM3 FBs，避免命令狀態因間歇呼叫失步
// Execute 旗標由狀態機或 Stop() 控制

// MC_Power（_xPowerOn 由狀態機與 Stop() 管理）
_fbMC_Power(Axis := Axis, Enable := TRUE, bRegulatorOn := _xPowerOn, bDriveStart := _xPowerOn);

// MC_Reset（Execute 由 TRY_RESET/NOT_READY 控制）
_fbMC_Reset(Axis := Axis);

// 運動命令 FBs（Execute 由狀態機控制）
_fbMC_MoveAbsolute(Axis := Axis);
_fbMC_MoveRelative(Axis := Axis);
_fbMC_MoveVelocity(Axis := Axis);
_fbMC_Stop(Axis := Axis);
_fbMC_Home(Axis := Axis);

// WriteHomingParameters（僅在 homing 寫參數流程呼叫）
IF iStep = enumAxisControl_Step.HOMING_WRITE_PARAM OR
	_fbMC_WriteHomingParameters.xExecute THEN
	_fbMC_WriteHomingParameters(Axis := Axis, xEnable := TRUE, stParameter := stParameter.stHoming);
END_IF

// EMS 緊急停止（xEMS=TRUE 時立即停止，Jerk=0 表示最快減速）
_fbMC_Stop_EMS(Axis := Axis, Execute := xEMS, Deceleration := stParameter.lrEmergencyStopDec, Jerk := 0);