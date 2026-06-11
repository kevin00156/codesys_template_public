METHOD Resume : BOOL

// === IMPLEMENTATION ===
IF xPaused THEN
    iStep := iLastStep;
    xPaused := FALSE;
    Resume := TRUE;
END_IF