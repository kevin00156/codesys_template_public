FUNCTION_BLOCK MC_BasicControl_VisuWrapper
VAR_IN_OUT
	fbAxis   : MC_BasicControl_LC10E;
	stCmd    : structMC_BasicControl_VisuCmd;
	stStatus : structMC_BasicControl_VisuStatus;
END_VAR
VAR
	_xClearLastResultPrev : BOOL;
	_xHomePrev            : BOOL;
	_xResetPrev           : BOOL;
	_xStopPrev            : BOOL;
	_xMoveAbsPrev         : BOOL;
	_xMoveRelPosPrev      : BOOL;
	_xMoveRelNegPrev      : BOOL;
	_xMoveVelPrev         : BOOL;
	_xSetPositionPrev     : BOOL;
	_xHomeLatched         : BOOL;
	_xMoveAbsLatched      : BOOL;
	_xMoveRelLatched      : BOOL;
	_xMoveVelLatched      : BOOL;
	_xSetPositionLatched  : BOOL;
	_udiLastPulseSeq   : UDINT;
END_VAR

// === IMPLEMENTATION ===
// Reject code:
// 1 = NONE command
// 2 = Unknown command
// 3 = Invalid motion parameter
stStatus.xPulseAccepted := FALSE;
stStatus.xPulseRejected := FALSE;
stStatus.iPulseRejectCode := 0;
stStatus.xJogCommanded := FALSE;

IF stCmd.xClearLastResult AND NOT _xClearLastResultPrev THEN
	Clear(stStatus.stLastResult);
END_IF

IF stCmd.xStop AND NOT _xStopPrev THEN
	fbAxis.Stop();
	_xHomeLatched := FALSE;
	_xMoveAbsLatched := FALSE;
	_xMoveRelLatched := FALSE;
	_xMoveVelLatched := FALSE;
	_xSetPositionLatched := FALSE;
	stStatus.xPulseAccepted := TRUE;
END_IF

IF stCmd.xReset AND NOT _xResetPrev THEN
	fbAxis.Reset();
	_xHomeLatched := FALSE;
	_xMoveAbsLatched := FALSE;
	_xMoveRelLatched := FALSE;
	_xMoveVelLatched := FALSE;
	_xSetPositionLatched := FALSE;
	stStatus.xPulseAccepted := TRUE;
END_IF

IF stCmd.udiPulseSeq <> _udiLastPulseSeq THEN
	CASE stCmd.ePulseCmd OF
		enumMC_BasicControl_VisuPulseCmd.STOP:
			fbAxis.Stop();
			_xHomeLatched := FALSE;
			_xMoveAbsLatched := FALSE;
			_xMoveRelLatched := FALSE;
			_xMoveVelLatched := FALSE;
			_xSetPositionLatched := FALSE;
			stStatus.xPulseAccepted := TRUE;
		enumMC_BasicControl_VisuPulseCmd.RESET:
			fbAxis.Reset();
			_xHomeLatched := FALSE;
			_xMoveAbsLatched := FALSE;
			_xMoveRelLatched := FALSE;
			_xMoveVelLatched := FALSE;
			_xSetPositionLatched := FALSE;
			stStatus.xPulseAccepted := TRUE;
		enumMC_BasicControl_VisuPulseCmd.HOME:
			_xHomeLatched := TRUE;
			_xMoveAbsLatched := FALSE;
			_xMoveRelLatched := FALSE;
			_xMoveVelLatched := FALSE;
			_xSetPositionLatched := FALSE;
			stStatus.xPulseAccepted := TRUE;
		enumMC_BasicControl_VisuPulseCmd.MOVE_ABSOLUTE:
			IF stCmd.lrMoveAbsVel < 0.0 OR stCmd.lrMoveAbsAcc < 0.0 OR stCmd.lrMoveAbsDec < 0.0 OR stCmd.lrMoveAbsJerk < 0.0 THEN
				stStatus.xPulseRejected := TRUE;
				stStatus.iPulseRejectCode := 3;
			ELSE
				_xHomeLatched := FALSE;
				_xMoveAbsLatched := TRUE;
				_xMoveRelLatched := FALSE;
				_xMoveVelLatched := FALSE;
				_xSetPositionLatched := FALSE;
				stStatus.xPulseAccepted := TRUE;
			END_IF
		enumMC_BasicControl_VisuPulseCmd.MOVE_RELATIVE:
			IF stCmd.lrMoveRelVel < 0.0 OR stCmd.lrMoveRelAcc < 0.0 OR stCmd.lrMoveRelDec < 0.0 OR stCmd.lrMoveRelJerk < 0.0 THEN
				stStatus.xPulseRejected := TRUE;
				stStatus.iPulseRejectCode := 3;
			ELSE
				_xHomeLatched := FALSE;
				_xMoveAbsLatched := FALSE;
				_xMoveRelLatched := TRUE;
				_xMoveVelLatched := FALSE;
				_xSetPositionLatched := FALSE;
				stStatus.xPulseAccepted := TRUE;
			END_IF
		enumMC_BasicControl_VisuPulseCmd.MOVE_VELOCITY:
			IF stCmd.lrMoveVelAcc < 0.0 OR stCmd.lrMoveVelDec < 0.0 OR stCmd.lrMoveVelJerk < 0.0 THEN
				stStatus.xPulseRejected := TRUE;
				stStatus.iPulseRejectCode := 3;
			ELSE
				_xHomeLatched := FALSE;
				_xMoveAbsLatched := FALSE;
				_xMoveRelLatched := FALSE;
				_xMoveVelLatched := TRUE;
				_xSetPositionLatched := FALSE;
				stStatus.xPulseAccepted := TRUE;
			END_IF
		enumMC_BasicControl_VisuPulseCmd.SET_POSITION:
			_xHomeLatched := FALSE;
			_xMoveAbsLatched := FALSE;
			_xMoveRelLatched := FALSE;
			_xMoveVelLatched := FALSE;
			_xSetPositionLatched := TRUE;
			stStatus.xPulseAccepted := TRUE;
		enumMC_BasicControl_VisuPulseCmd.NONE:
			stStatus.xPulseRejected := TRUE;
			stStatus.iPulseRejectCode := 1;
	ELSE
		stStatus.xPulseRejected := TRUE;
		stStatus.iPulseRejectCode := 2;
	END_CASE

	_udiLastPulseSeq      := stCmd.udiPulseSeq;
	stStatus.udiPulseAckSeq := stCmd.udiPulseSeq;
	stStatus.ePulseAckCmd   := stCmd.ePulseCmd;
END_IF

// Button pulse compatibility path for visu widgets.
IF stCmd.xHome AND NOT _xHomePrev THEN
	_xHomeLatched := TRUE;
	_xMoveAbsLatched := FALSE;
	_xMoveRelLatched := FALSE;
	_xMoveVelLatched := FALSE;
	_xSetPositionLatched := FALSE;
	stStatus.xPulseAccepted := TRUE;
ELSIF stCmd.xMoveAbsoluteExecute AND NOT _xMoveAbsPrev THEN
	IF stCmd.lrMoveAbsVel < 0.0 OR stCmd.lrMoveAbsAcc < 0.0 OR stCmd.lrMoveAbsDec < 0.0 OR stCmd.lrMoveAbsJerk < 0.0 THEN
		stStatus.xPulseRejected := TRUE;
		stStatus.iPulseRejectCode := 3;
	ELSE
		_xHomeLatched := FALSE;
		_xMoveAbsLatched := TRUE;
		_xMoveRelLatched := FALSE;
		_xMoveVelLatched := FALSE;
		_xSetPositionLatched := FALSE;
		stStatus.xPulseAccepted := TRUE;
	END_IF
ELSIF stCmd.xMoveRelativePosExecute AND NOT _xMoveRelPosPrev THEN
	IF stCmd.lrMoveRelVel < 0.0 OR stCmd.lrMoveRelAcc < 0.0 OR stCmd.lrMoveRelDec < 0.0 OR stCmd.lrMoveRelJerk < 0.0 THEN
		stStatus.xPulseRejected := TRUE;
		stStatus.iPulseRejectCode := 3;
	ELSE
		stCmd.lrMoveRelDist := ABS(stCmd.lrMoveRelDist);
		_xHomeLatched := FALSE;
		_xMoveAbsLatched := FALSE;
		_xMoveRelLatched := TRUE;
		_xMoveVelLatched := FALSE;
		_xSetPositionLatched := FALSE;
		stStatus.xPulseAccepted := TRUE;
	END_IF
ELSIF stCmd.xMoveRelativeNegExecute AND NOT _xMoveRelNegPrev THEN
	IF stCmd.lrMoveRelVel < 0.0 OR stCmd.lrMoveRelAcc < 0.0 OR stCmd.lrMoveRelDec < 0.0 OR stCmd.lrMoveRelJerk < 0.0 THEN
		stStatus.xPulseRejected := TRUE;
		stStatus.iPulseRejectCode := 3;
	ELSE
		stCmd.lrMoveRelDist := -ABS(stCmd.lrMoveRelDist);
		_xHomeLatched := FALSE;
		_xMoveAbsLatched := FALSE;
		_xMoveRelLatched := TRUE;
		_xMoveVelLatched := FALSE;
		_xSetPositionLatched := FALSE;
		stStatus.xPulseAccepted := TRUE;
	END_IF
ELSIF stCmd.xMoveVelocityExecute AND NOT _xMoveVelPrev THEN
	IF stCmd.lrMoveVelAcc < 0.0 OR stCmd.lrMoveVelDec < 0.0 OR stCmd.lrMoveVelJerk < 0.0 THEN
		stStatus.xPulseRejected := TRUE;
		stStatus.iPulseRejectCode := 3;
	ELSE
		_xHomeLatched := FALSE;
		_xMoveAbsLatched := FALSE;
		_xMoveRelLatched := FALSE;
		_xMoveVelLatched := TRUE;
		_xSetPositionLatched := FALSE;
		stStatus.xPulseAccepted := TRUE;
	END_IF
ELSIF stCmd.xSetPosition AND NOT _xSetPositionPrev THEN
	_xHomeLatched := FALSE;
	_xMoveAbsLatched := FALSE;
	_xMoveRelLatched := FALSE;
	_xMoveVelLatched := FALSE;
	_xSetPositionLatched := TRUE;
	stStatus.xPulseAccepted := TRUE;
END_IF

IF _xHomeLatched THEN
	stStatus.stLastResult := fbAxis.Home();
	IF stStatus.stLastResult.xDone OR stStatus.stLastResult.xError THEN
		_xHomeLatched := FALSE;
	END_IF
ELSIF _xMoveAbsLatched THEN
	stStatus.stLastResult := fbAxis.MoveAbsolute(
		lrPosition := stCmd.lrMoveAbsPos,
		lrVelocity := stCmd.lrMoveAbsVel,
		lrAcceleration := stCmd.lrMoveAbsAcc,
		lrDeceleration := stCmd.lrMoveAbsDec,
		lrJerk := stCmd.lrMoveAbsJerk
	);
	IF stStatus.stLastResult.xDone OR stStatus.stLastResult.xError THEN
		_xMoveAbsLatched := FALSE;
	END_IF
ELSIF _xMoveRelLatched THEN
	stStatus.stLastResult := fbAxis.MoveRelative(
		lrDistance := stCmd.lrMoveRelDist,
		lrVelocity := stCmd.lrMoveRelVel,
		lrAcceleration := stCmd.lrMoveRelAcc,
		lrDeceleration := stCmd.lrMoveRelDec,
		lrJerk := stCmd.lrMoveRelJerk
	);
	IF stStatus.stLastResult.xDone OR stStatus.stLastResult.xError THEN
		_xMoveRelLatched := FALSE;
	END_IF
ELSIF _xMoveVelLatched THEN
	stStatus.stLastResult := fbAxis.MoveVelocity(
		lrVelocity := stCmd.lrMoveVel,
		lrAcceleration := stCmd.lrMoveVelAcc,
		lrDeceleration := stCmd.lrMoveVelDec,
		lrJerk := stCmd.lrMoveVelJerk
	);
	IF stStatus.stLastResult.xError THEN
		_xMoveVelLatched := FALSE;
	END_IF
ELSIF _xSetPositionLatched THEN
	stStatus.stLastResult := fbAxis.SetPosition(lrPosition := stCmd.lrSetPosition);
	IF stStatus.stLastResult.xDone OR stStatus.stLastResult.xError THEN
		_xSetPositionLatched := FALSE;
	END_IF
END_IF

// Level command path (jog): hold TRUE to keep command active.
IF NOT (_xHomeLatched OR _xMoveAbsLatched OR _xMoveRelLatched OR _xMoveVelLatched OR _xSetPositionLatched) THEN
	IF stCmd.xJogPositive XOR stCmd.xJogNegative THEN
	IF stCmd.lrJogVelocity < 0.0 OR stCmd.lrJogAcc < 0.0 OR stCmd.lrJogDec < 0.0 OR stCmd.lrJogJerk < 0.0 THEN
		stStatus.xPulseRejected := TRUE;
		stStatus.iPulseRejectCode := 3;
	ELSIF stCmd.xJogPositive THEN
		stStatus.stLastResult := fbAxis.JogPositive(
			lrVelocity := stCmd.lrJogVelocity,
			lrAcceleration := stCmd.lrJogAcc,
			lrDeceleration := stCmd.lrJogDec,
			lrJerk := stCmd.lrJogJerk
		);
		stStatus.xJogCommanded := TRUE;
	ELSE
		stStatus.stLastResult := fbAxis.JogNegative(
			lrVelocity := stCmd.lrJogVelocity,
			lrAcceleration := stCmd.lrJogAcc,
			lrDeceleration := stCmd.lrJogDec,
			lrJerk := stCmd.lrJogJerk
		);
		stStatus.xJogCommanded := TRUE;
	END_IF
	END_IF
END_IF

IF stCmd.xRaiseDummyError THEN
	fbAxis.Raise(
		iErrorID:=enumAxisControl_ErrorID.DUMMY,
		iNextStep:=fbAxis.iStep
	);
END_IF

_xClearLastResultPrev := stCmd.xClearLastResult;
_xHomePrev := stCmd.xHome;
_xResetPrev := stCmd.xReset;
_xStopPrev := stCmd.xStop;
_xMoveAbsPrev := stCmd.xMoveAbsoluteExecute;
_xMoveRelPosPrev := stCmd.xMoveRelativePosExecute;
_xMoveRelNegPrev := stCmd.xMoveRelativeNegExecute;
_xMoveVelPrev := stCmd.xMoveVelocityExecute;
_xSetPositionPrev := stCmd.xSetPosition;

stStatus.xEnabled              := fbAxis.xEnabled;
stStatus.xBusy                 := fbAxis.xBusy;
stStatus.xDone                 := fbAxis.xDone;
stStatus.xError                := fbAxis.xError;
stStatus.iErrorID              := fbAxis.iErrorID;
stStatus.iStep                 := fbAxis.iStep;
stStatus.xAxisStandStill       := fbAxis.xAxisStandStill;
stStatus.xInVelocity           := fbAxis.xInVelocity;
stStatus.xInAcceleration       := FALSE;
stStatus.xInDeceleration       := FALSE;
stStatus.xPositiveLimitReached := fbAxis.xPositiveLimitReached;
stStatus.xNegativeLimitReached := fbAxis.xNegativeLimitReached;
stStatus.xBrakeSignal          := FALSE;

IF fbAxis.pAxis <> 0 THEN
	stStatus.lrActPosition := fbAxis.pAxis^.fActPosition;
	stStatus.lrActVelocity := fbAxis.pAxis^.fActVelocity;
	stStatus.lrSetPosition := fbAxis.pAxis^.fSetPosition;
	stStatus.lrSetVelocity := fbAxis.pAxis^.fSetVelocity;
END_IF