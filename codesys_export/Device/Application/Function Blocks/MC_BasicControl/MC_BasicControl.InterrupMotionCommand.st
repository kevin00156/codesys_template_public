METHOD PUBLIC InterrupMotionCommand : BOOL
VAR_INPUT
END_VAR

// === IMPLEMENTATION ===
iStep := enumAxisControl_Step.STOPPING;  // 中斷當前動作，進入停止序列
InterrupMotionCommand := TRUE;