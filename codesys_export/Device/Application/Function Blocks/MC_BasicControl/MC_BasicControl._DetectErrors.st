METHOD PRIVATE _DetectErrors

// === IMPLEMENTATION ===
// 偵測驅動器錯誤碼（已由 ReadDriverInputs 過濾已知正常碼）
// 觸發 Raise → 進入錯誤流程，TRY_RESET 後恢復
IF _wDriverErrorCode <> 0 THEN
	_ClearAllExecute();
	_eLastMotionError := THIS^._wDriverErrorCode;
	Raise(
		iErrorID  := enumAxisControl_ErrorID.DRIVER_ERROR,
		iNextStep := enumAxisControl_Step.TRY_RESET);
END_IF