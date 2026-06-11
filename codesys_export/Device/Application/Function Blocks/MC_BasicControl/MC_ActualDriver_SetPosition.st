// X57s 驅動器不支援標準 MC_SetPosition（Set Position 指令）
// 使用 Homing Mode 35 模擬 SetPosition 的功能作為 Workaround
FUNCTION_BLOCK MC_ActualDriver_SetPosition
VAR_IN_OUT
	Axis : AXIS_REF_SM3;
END_VAR
VAR_INPUT
	Execute		: BOOL;
	Position	: LREAL;
	xReset		: BOOL;
END_VAR
VAR_OUTPUT
	Done	: BOOL;
	Busy	: BOOL;
	Error	: BOOL;
	ErrorID	: SM3_Basic.SMC_ERROR;
END_VAR
VAR
	iStep : INT;
	_fbMC_WriteHomingParameters : MC_WriteHomingParameters;
	_stFakeHomingParameter : structAxisControl_HomingProfile := (uiHomingMethod := 35);
	_fbMC_Home : MC_Home;
END_VAR

// === IMPLEMENTATION ===
_fbMC_WriteHomingParameters(Axis := Axis, xEnable := TRUE, stParameter := _stFakeHomingParameter);
_fbMC_Home(Axis := Axis, Position := Position);

CASE iStep OF
	0: // 等待 Execute
	_fbMC_WriteHomingParameters.xExecute := FALSE;
	_fbMC_Home.Execute := FALSE;
	Done := FALSE;
	Error := FALSE;
	Busy := FALSE;
	IF Execute THEN
		iStep := iStep + 1;
	END_IF

	1: // 寫入 Homing 參數（Homing Mode 35）
	_fbMC_WriteHomingParameters.xExecute := TRUE;
	Busy := TRUE;
	IF _fbMC_WriteHomingParameters.xDone THEN
		_fbMC_WriteHomingParameters.xExecute := FALSE;
		iStep := iStep + 1;
	END_IF
	IF _fbMC_WriteHomingParameters.xError THEN
		Error := TRUE;
		iStep := 9999;
	END_IF

	2: // 執行 MC_Home（使用 Mode 35 觸發位置設定）
	_fbMC_Home.Execute := TRUE;
	IF _fbMC_Home.Done THEN
		_fbMC_Home.Execute := FALSE;
		iStep := iStep + 1;
	END_IF
	IF _fbMC_Home.Error THEN
		Error := TRUE;
		ErrorID := _fbMC_Home.ErrorID;
		iStep := 9999;
	END_IF

ELSE // iStep >= 3 or 9999
	Done := (iStep <> 9999);
	IF NOT Execute THEN
		iStep := 0;
		Done := FALSE;
		Error := FALSE;
		ErrorID := 0;
	END_IF
END_CASE

IF xReset THEN
	iStep := 0;
	Done := FALSE;
	Error := FALSE;
	ErrorID := 0;
END_IF