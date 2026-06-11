METHOD Home : structAxisControl_Result

// === IMPLEMENTATION ===
// 設定請求旗標（每掃描週期持續呼叫直到 Done/Error）
_xHomeRequested := TRUE;

// 回傳當前狀態
Home.xBusy  := (iStep = enumAxisControl_Step.HOMING_WRITE_PARAM) OR
               (iStep = enumAxisControl_Step.HOMING_EXEC);
Home.xDone  := _xCommandDone AND (_iActiveCommand = enumAxisControl_Step.HOMING_EXEC);
Home.xError := xError;
Home.eErrorID := _eLastMotionError;