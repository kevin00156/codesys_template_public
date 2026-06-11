TYPE structMC_BasicControl_VisuStatus :
STRUCT
	xEnabled               : BOOL;
	xBusy                  : BOOL;
	xDone                  : BOOL;
	xError                 : BOOL;
	iErrorID               : INT;
	xPulseAccepted         : BOOL;
	xPulseRejected         : BOOL;
	iPulseRejectCode       : INT;
	udiPulseAckSeq         : UDINT;
	ePulseAckCmd           : enumMC_BasicControl_VisuPulseCmd;
	xPulseLatched          : BOOL;
	ePulseLatchedCmd       : enumMC_BasicControl_VisuPulseCmd;
	iPulseLatchedCmd       : INT;
	xJogCommanded          : BOOL;

	xAxisStandStill        : BOOL;
	xInVelocity            : BOOL;
	xInAcceleration        : BOOL;
	xInDeceleration        : BOOL;
	xPositiveLimitReached  : BOOL;
	xNegativeLimitReached  : BOOL;
	xBrakeSignal           : BOOL;

	lrActPosition          : LREAL;
	lrActVelocity          : LREAL;
	lrSetPosition          : LREAL;
	lrSetVelocity          : LREAL;

	stLastResult           : structAxisControl_Result;
	
	iStep					: INT;
END_STRUCT
END_TYPE