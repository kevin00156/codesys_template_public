METHOD Reset : BOOL

// === IMPLEMENTATION ===
IF xError THEN
    xError := FALSE;
    iErrorID := 0;
    iStep := iLastStep;
    Reset := TRUE;
END_IF