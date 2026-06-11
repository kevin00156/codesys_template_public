METHOD Raise : BOOL
VAR_INPUT 
	iErrorID : INT; 
	iNextStep : INT;
END_VAR

// === IMPLEMENTATION ===
IF NOT xError AND NOT xPaused THEN
    THIS^.iErrorID := iErrorID;
    xError := TRUE;
	_iRaiseStep := iStep;
	IF iLastStep < enumStateMachine_States.DELAY_FOR_NEXT_STEP THEN
    	iLastStep := iNextStep; // 紀錄出錯地點
	END_IF
    iStep := enumStateMachine_States.ERROR;      // 進入基底錯誤區
    Raise := TRUE;
END_IF