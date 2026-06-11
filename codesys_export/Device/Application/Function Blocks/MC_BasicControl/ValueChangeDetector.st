FUNCTION_BLOCK ValueChangeDetector
VAR_INPUT
	In		: LREAL;
END_VAR
VAR_OUTPUT
	xValueChange_RisingTrig : BOOL;
END_VAR
VAR
	lrTemp : LREAL;
	rtValueChangeDetector : R_TRIG;
END_VAR

// === IMPLEMENTATION ===
rtValueChangeDetector(CLK:=In<>lrTemp,Q=>xValueChange_RisingTrig);
lrTemp:=In;