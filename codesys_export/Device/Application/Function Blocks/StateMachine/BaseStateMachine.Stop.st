METHOD Stop : BOOL

// === IMPLEMENTATION ===
iStep := 0;
xError := FALSE;
xPaused := FALSE;
iErrorID := 0;
Stop := TRUE;
xBusy:=FALSE;
xDone:=FALSE;