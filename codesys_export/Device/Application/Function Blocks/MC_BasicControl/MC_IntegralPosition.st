FUNCTION_BLOCK MC_IntegralPosition
VAR_IN_OUT
	Axis : AXIS_REF_SM3;
END_VAR
VAR_INPUT
	xEnable	: BOOL;
	xReset	: BOOL;
	
END_VAR
VAR_OUTPUT
	xBusy			: BOOL;
	lrCmdPosition	: LREAL;
	lrActPosition	: LREAL;
END_VAR
VAR
	lrScanTime	: LREAL;//sec
END_VAR

// === IMPLEMENTATION ===
IF xEnable THEN
	
	lrScanTime:=Axis.fTaskCycle;
	
	IF NOT xReset THEN
		lrCmdPosition:=lrCmdPosition+Axis.fSetVelocity*lrScanTime;
		lrActPosition:=lrActPosition+Axis.fActVelocity*lrScanTime;
	ELSE
		lrCmdPosition:=0;
		lrActPosition:=0;
	END_IF
END_IF