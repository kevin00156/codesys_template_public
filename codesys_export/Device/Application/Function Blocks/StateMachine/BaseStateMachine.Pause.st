METHOD Pause : BOOL

// === IMPLEMENTATION ===
IF NOT xError AND NOT xPaused THEN
    iLastStep := iStep;
    iStep := enumStateMachine_States.PAUSE; // 進入基類暫停區
    xPaused := TRUE;
    Pause := TRUE;
END_IF