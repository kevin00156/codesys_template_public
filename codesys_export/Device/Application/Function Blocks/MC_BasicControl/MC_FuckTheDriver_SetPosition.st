FUNCTION_BLOCK MC_FuckTheDriver_SetPosition
VAR_IN_OUT
	Axis : AXIS_REF_SM3;
END_VAR
VAR_INPUT
	Execute : BOOL;
	Position : LREAL;
	Mode : BOOL;
	xReset : BOOL;
END_VAR
VAR_OUTPUT
	Done : BOOL;
	Busy : BOOL;
	Error : BOOL;
	ErrorID : SM3_Basic.SMC_ERROR;
END_VAR
VAR
	iStep : INT;
	fbMC_WriteHomingParameters : MC_WriteHomingParameters;
	FakeHomingParameter : structAxisControl_HomingProfile :=
	(uiHomingMethod:=35);
	fbMC_Home : MC_Home;
END_VAR

// === IMPLEMENTATION ===
fbMC_WriteHomingParameters(Axis:=Axis,xEnable:=TRUE,stParameter:=FakeHomingParameter);
fbMC_Home(Axis:=Axis, Position:=Position);
CASE iStep OF
	0:
	fbMC_WriteHomingParameters.xExecute:=FALSE;
	fbMC_Home.Execute:=FALSE;
	IF Execute THEN
		iStep:=iStep+1;
	END_IF
	1:
	fbMC_WriteHomingParameters.xExecute:=TRUE;
	IF fbMC_WriteHomingParameters.xDone THEN
		fbMC_WriteHomingParameters.xExecute:=FALSE;
		iStep:=iStep+1;
	END_IF
	IF fbMC_WriteHomingParameters.xError THEN
		iStep:=9999;
		
	END_IF
	2:
	fbMC_Home.Execute:=TRUE;
	IF fbMC_Home.Done THEN
		fbMC_Home.Execute:=FALSE;
		iStep:=iStep+1;
	END_IF
	IF fbMC_Home.Error THEN
		iStep:=9999;
		Error:=TRUE;
		ErrorID:=fbMC_Home.ErrorID;
	END_IF
ELSE
	Done:=iStep<>9999;
	IF NOT Execute THEN
		iStep:=0;
		Done:=0;
		Error:=0;
		ErrorID:=0;
	END_IF
END_CASE
IF xReset THEN
	iStep:=0;
END_IF