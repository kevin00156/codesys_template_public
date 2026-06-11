METHOD Reset : BOOL

// === IMPLEMENTATION ===
// 覆蓋 BaseStateMachine.Reset()：
// 不恢復 iLastStep，而是進入 TRY_RESET 讓驅動器確實重置後再恢復 READY
IF xError THEN
	_ClearAllExecute();
	_fbMC_Stop.Execute  := FALSE;
	_fbMC_Reset.Execute := FALSE;
	iStep               := enumAxisControl_Step.TRY_RESET;
	_iStepBeforeReset   := enumAxisControl_Step.READY;
	xError              := FALSE;
	iErrorID            := 0;
	Reset               := TRUE;
END_IF
// 不呼叫 SUPER^.Reset()