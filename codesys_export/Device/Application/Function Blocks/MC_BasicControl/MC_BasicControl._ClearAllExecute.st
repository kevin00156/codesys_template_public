METHOD PROTECTED _ClearAllExecute

// === IMPLEMENTATION ===
// 清除所有運動命令 FBs 的 Execute 旗標
// 不清除 MC_Stop（由 STOPPING/WAIT_STOP 狀態管理）
// 不清除 MC_Reset（由 TRY_RESET 狀態管理）
// 不清除 MC_Stop_EMS（由 xEMS 輸入直接控制）
_fbMC_MoveAbsolute.Execute           := FALSE;
_fbMC_MoveRelative.Execute           := FALSE;
_fbMC_MoveVelocity.Execute           := FALSE;
_fbMC_Home.Execute                   := FALSE;
_fbMC_WriteHomingParameters.xExecute := FALSE;