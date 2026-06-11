METHOD PRIVATE _ClearAndSaveRequestFlags

// === IMPLEMENTATION ===
// 將當前掃描的請求旗標儲存至 _Prev 版本（供狀態機使用）
// 然後清除當前旗標（等待本掃描週期的 method 呼叫重新設定）
_xJogPosRequested_Prev  := _xJogPosRequested;
_xJogNegRequested_Prev  := _xJogNegRequested;
_xMoveAbsRequested_Prev := _xMoveAbsRequested;
_xMoveRelRequested_Prev := _xMoveRelRequested;
_xMoveVelRequested_Prev := _xMoveVelRequested;
_xHomeRequested_Prev    := _xHomeRequested;
_xSetPosRequested_Prev  := _xSetPosRequested;

_xJogPosRequested  := FALSE;
_xJogNegRequested  := FALSE;
_xMoveAbsRequested := FALSE;
_xMoveRelRequested := FALSE;
_xMoveVelRequested := FALSE;
_xHomeRequested    := FALSE;
_xSetPosRequested  := FALSE;