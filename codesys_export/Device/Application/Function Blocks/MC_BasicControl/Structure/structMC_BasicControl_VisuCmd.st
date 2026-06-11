TYPE structMC_BasicControl_VisuCmd :
STRUCT
	xRegulatorOn     : BOOL;
	xHome            : BOOL;
	xReset           : BOOL;
	xStop            : BOOL;
	xMoveAbsoluteExecute    : BOOL;
	xMoveRelativePosExecute : BOOL;
	xMoveRelativeNegExecute : BOOL;
	xMoveVelocityExecute    : BOOL;
	xSetPosition     : BOOL;
	xJogPositive     : BOOL;
	xJogNegative     : BOOL;

	// Pulse command path. Increase udiPulseSeq to issue exactly one command.
	ePulseCmd        : enumMC_BasicControl_VisuPulseCmd;
	udiPulseSeq      : UDINT;

	lrJogVelocity    : LREAL;
	lrJogAcc         : LREAL;
	lrJogDec         : LREAL;
	lrJogJerk        : LREAL;

	lrMoveAbsPos     : LREAL;
	lrMoveAbsVel     : LREAL;
	lrMoveAbsAcc     : LREAL;
	lrMoveAbsDec     : LREAL;
	lrMoveAbsJerk    : LREAL;

	lrMoveRelDist    : LREAL;
	lrMoveRelVel     : LREAL;
	lrMoveRelAcc     : LREAL;
	lrMoveRelDec     : LREAL;
	lrMoveRelJerk    : LREAL;

	lrMoveVel        : LREAL;
	lrMoveVelAcc     : LREAL;
	lrMoveVelDec     : LREAL;
	lrMoveVelJerk    : LREAL;
	lrStopDec        : LREAL;

	lrSetPosition    : LREAL;
	uiHomingMethod   : UINT;
	lrUnitToPulseRatio : LREAL;
	lrHomingPositionOffset : LREAL;
	xClearLastResult : BOOL;
	
	xRaiseDummyError : BOOL;
END_STRUCT
END_TYPE