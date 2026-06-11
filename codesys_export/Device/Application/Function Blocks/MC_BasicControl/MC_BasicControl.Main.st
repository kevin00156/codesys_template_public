METHOD PROTECTED Main
VAR
	_iDoSetPosResult : INT;
END_VAR

// === IMPLEMENTATION ===
// ─── 1. 儲存並清除命令請求旗標 ────────────────────────────────────────────
// 將前一掃描週期 method 設定的旗標存入 _Prev，清除現旗標等待本週期重設
_ClearAndSaveRequestFlags();

// ─── 2. 讀取驅動器 IO（Abstract） ────────────────────────────────────────
// 子類填入 _xPositiveLimitInput, _xNegativeLimitInput, _xBrakeInput, _wDriverErrorCode
// 並更新 xPositiveLimitReached, xNegativeLimitReached
ReadDriverInputs();

// ─── 3. 更新輸出旗標 ──────────────────────────────────────────────────────
xAxisStandStill := Axis.nAxisState=SM3_Basic.SMC_AXIS_STATE.standstill;
xInVelocity     := Axis.bConstantVelocity;

// ─── 4. 偵測驅動器錯誤 ───────────────────────────────────────────────────
_DetectErrors();
IF xError THEN RETURN; END_IF

// ─── 5. 緊急停止（EMS） ──────────────────────────────────────────────────
// MC_Stop_EMS 已在 _RunMotionFBs() 中根據 xEMS 執行
// 此處負責清除其他命令並將狀態機移至安全狀態
IF xEMS THEN
	_ClearAllExecute();
	_fbMC_Stop.Execute := FALSE;
	IF iStep = enumAxisControl_Step.JOG_POS       OR
	   iStep = enumAxisControl_Step.JOG_NEG       OR
	   iStep = enumAxisControl_Step.MOVE_ABS      OR
	   iStep = enumAxisControl_Step.MOVE_REL      OR
	   iStep = enumAxisControl_Step.MOVE_VEL      OR
	   iStep = enumAxisControl_Step.HOMING_WRITE_PARAM OR
	   iStep = enumAxisControl_Step.HOMING_EXEC   OR
	   iStep = enumAxisControl_Step.SET_POSITION THEN
		iStep := enumAxisControl_Step.STOPPING;
	END_IF
	RETURN;
END_IF

// ─── 6. 狀態機 ────────────────────────────────────────────────────────────
CASE iStep OF

	// ── IDLE：等待 xEnable ──────────────────────────────────────────────
	enumAxisControl_Step.IDLE:
		_xPowerOn  := FALSE;
		xBusy      := FALSE;
		_xCommandDone := FALSE;
		IF xEnable THEN
			iStep := enumAxisControl_Step.ENABLING;
		END_IF

	// ── ENABLING：等待伺服使能完成 ─────────────────────────────────────
	enumAxisControl_Step.ENABLING:
		_xPowerOn := TRUE;
		xBusy     := TRUE;
		IF Axis.bRegulatorOn THEN
			iStep := enumAxisControl_Step.READY;
		ELSIF Axis.bError THEN
			iStep := enumAxisControl_Step.NOT_READY;
		END_IF

	// ── NOT_READY：軸有錯，嘗試 Reset 後再進入 READY ──────────────────
	enumAxisControl_Step.NOT_READY:
		_xPowerOn := TRUE;
		xBusy     := TRUE;
		_fbMC_Reset.Execute := TRUE;
		IF _fbMC_Reset.Done OR _fbMC_Reset.Error THEN
			_fbMC_Reset.Execute := FALSE;
		END_IF
		IF Axis.nAxisState=SM3_Basic.SMC_AXIS_STATE.standstill AND NOT Axis.bError THEN
			iStep := enumAxisControl_Step.READY;
		END_IF

	// ── READY：等待命令 ────────────────────────────────────────────────
	enumAxisControl_Step.READY:
		_xPowerOn     := TRUE;
		xBusy         := FALSE;
		_xCommandDone := FALSE;
		// MoveRelative 解鎖：呼叫端停止呼叫時清除重觸旗標
		IF NOT _xMoveRelRequested_Prev THEN
			_xMoveRelNeedsReArm := FALSE;
		END_IF
		// 命令分派（優先順序由上而下）
		IF _xMoveAbsRequested_Prev THEN
			iStep := enumAxisControl_Step.MOVE_ABS;
		ELSIF _xMoveRelRequested_Prev AND NOT _xMoveRelNeedsReArm THEN
			iStep := enumAxisControl_Step.MOVE_REL;
		ELSIF _xMoveVelRequested_Prev THEN
			iStep := enumAxisControl_Step.MOVE_VEL;
		ELSIF (_xJogPosRequested_Prev OR xJogPositive) AND NOT _xPositiveLimitInput THEN
			iStep := enumAxisControl_Step.JOG_POS;
		ELSIF (_xJogNegRequested_Prev OR xJogNegative) AND NOT _xNegativeLimitInput THEN
			iStep := enumAxisControl_Step.JOG_NEG;
		ELSIF _xHomeRequested_Prev THEN
			iStep := enumAxisControl_Step.HOMING_WRITE_PARAM;
		ELSIF _xSetPosRequested_Prev THEN
			iStep := enumAxisControl_Step.SET_POSITION;
		END_IF

	// ── JOG_POS：正方向點動 ────────────────────────────────────────────
	enumAxisControl_Step.JOG_POS:
		xBusy := TRUE;
		_fbMC_MoveVelocity.Direction	:= SM3_Basic.MC_DIRECTION.positive;
		// ContinuousUpdate 模式：TRUE=持續刷新 Execute；FALSE=速度變化時重啟
		IF stParameter.xContinuousUpdate THEN
			_fbMC_MoveVelocity.Execute := TRUE;
		ELSE
			_fbVCD_Jog(In := _lrJogVelocity);
			IF _fbVCD_Jog.xValueChange_RisingTrig THEN
				_fbMC_MoveVelocity.Execute := FALSE;	// 短暫降低，下一掃描重啟
			ELSE
				_fbMC_MoveVelocity.Execute := TRUE;
			END_IF
		END_IF
		// 正向極限觸動：停止並嘗試重置
		IF _xPositiveLimitInput THEN
			_fbMC_MoveVelocity.Execute := FALSE;
			Raise(iErrorID  := enumAxisControl_ErrorID.POSITIVE_LIMIT_TRIP,
			      iNextStep := enumAxisControl_Step.TRY_RESET);
			RETURN;
		END_IF
		// 命令取消（method 停止呼叫）
		IF NOT (_xJogPosRequested_Prev OR xJogPositive) THEN
			_fbMC_MoveVelocity.Execute := FALSE;
			iStep := enumAxisControl_Step.STOPPING;
		END_IF
		IF _fbMC_MoveVelocity.Error THEN
			_fbMC_MoveVelocity.Execute := FALSE;
			_eLastMotionError := _fbMC_MoveVelocity.ErrorID;
			Raise(iErrorID  := enumAxisControl_ErrorID.DRIVER_ERROR,
			      iNextStep := enumAxisControl_Step.TRY_RESET);
			RETURN;
		END_IF

	// ── JOG_NEG：負方向點動 ────────────────────────────────────────────
	enumAxisControl_Step.JOG_NEG:
		xBusy := TRUE;
		_fbMC_MoveVelocity.Direction	:= SM3_Basic.MC_DIRECTION.negative;
		// ContinuousUpdate 邏輯（追蹤絕對速度值）
		IF stParameter.xContinuousUpdate THEN
			_fbMC_MoveVelocity.Execute := TRUE;
		ELSE
			_fbVCD_Jog(In := _lrJogVelocity);
			IF _fbVCD_Jog.xValueChange_RisingTrig THEN
				_fbMC_MoveVelocity.Execute := FALSE;
			ELSE
				_fbMC_MoveVelocity.Execute := TRUE;
			END_IF
		END_IF
		// 負向極限觸動
		IF _xNegativeLimitInput THEN
			_fbMC_MoveVelocity.Execute := FALSE;
			Raise(iErrorID  := enumAxisControl_ErrorID.NEGATIVE_LIMIT_TRIP,
			      iNextStep := enumAxisControl_Step.TRY_RESET);
			RETURN;
		END_IF
		// 命令取消
		IF NOT (_xJogNegRequested_Prev OR xJogNegative) THEN
			_fbMC_MoveVelocity.Execute := FALSE;
			iStep := enumAxisControl_Step.STOPPING;
		END_IF
		IF _fbMC_MoveVelocity.Error THEN
			_fbMC_MoveVelocity.Execute := FALSE;
			_eLastMotionError := _fbMC_MoveVelocity.ErrorID;
			Raise(iErrorID  := enumAxisControl_ErrorID.DRIVER_ERROR,
			      iNextStep := enumAxisControl_Step.TRY_RESET);
			RETURN;
		END_IF

	// ── MOVE_ABS：絕對位置移動 ────────────────────────────────────────
	enumAxisControl_Step.MOVE_ABS:
		xBusy := TRUE;
		_fbMC_MoveAbsolute.Execute      := TRUE;
		IF _fbMC_MoveAbsolute.Done THEN
			_xCommandDone   := TRUE;
			_iActiveCommand := enumAxisControl_Step.MOVE_ABS;
			_ClearAllExecute();
			iStep := enumAxisControl_Step.STOPPING;
		ELSIF _fbMC_MoveAbsolute.Error THEN
			_ClearAllExecute();
			_eLastMotionError := _fbMC_MoveAbsolute.ErrorID;
			Raise(iErrorID  := enumAxisControl_ErrorID.MOVE_ABS_FAIL,
			      iNextStep := enumAxisControl_Step.READY);
			RETURN;
		ELSIF NOT _xMoveAbsRequested_Prev THEN
			// 呼叫端停止呼叫 MoveAbsolute() → 取消
			_ClearAllExecute();
			iStep := enumAxisControl_Step.STOPPING;
		END_IF

	// ── MOVE_REL：相對位置移動 ────────────────────────────────────────
	enumAxisControl_Step.MOVE_REL:
		xBusy := TRUE;
		_fbMC_MoveRelative.Execute      := TRUE;
		IF _fbMC_MoveRelative.Done THEN
			_xCommandDone       := TRUE;
			_iActiveCommand     := enumAxisControl_Step.MOVE_REL;
			_xMoveRelNeedsReArm := TRUE;	// 鎖住：防止呼叫端持續呼叫時立刻重新執行
			_ClearAllExecute();
			iStep := enumAxisControl_Step.STOPPING;
		ELSIF _fbMC_MoveRelative.Error THEN
			_ClearAllExecute();
			_eLastMotionError := _fbMC_MoveRelative.ErrorID;
			Raise(iErrorID  := enumAxisControl_ErrorID.MOVE_REL_FAIL,
			      iNextStep := enumAxisControl_Step.READY);
			RETURN;
		ELSIF NOT _xMoveRelRequested_Prev THEN
			_ClearAllExecute();
			iStep := enumAxisControl_Step.STOPPING;
		END_IF

	// ── MOVE_VEL：速度模式（持續）───────────────────────────────────
	enumAxisControl_Step.MOVE_VEL:
		xBusy := TRUE;
		_fbMC_MoveVelocity.Execute      := TRUE;
		IF _fbMC_MoveVelocity.Error THEN
			_ClearAllExecute();
			_eLastMotionError := _fbMC_MoveVelocity.ErrorID;
			Raise(iErrorID  := enumAxisControl_ErrorID.MOVE_VEL_FAIL,
			      iNextStep := enumAxisControl_Step.READY);
			RETURN;
		ELSIF NOT _xMoveVelRequested_Prev THEN
			// 呼叫端停止呼叫 MoveVelocity() → 停止
			_fbMC_MoveVelocity.Execute := FALSE;
			iStep := enumAxisControl_Step.STOPPING;
		END_IF

	// ── HOMING_WRITE_PARAM：寫入回原點驅動器參數 ─────────────────────
	enumAxisControl_Step.HOMING_WRITE_PARAM:
		xBusy := TRUE;
		_fbMC_WriteHomingParameters.xExecute := TRUE;
		IF _fbMC_WriteHomingParameters.xDone THEN
			_fbMC_WriteHomingParameters.xExecute := FALSE;
			iStep := enumAxisControl_Step.HOMING_EXEC;
		ELSIF _fbMC_WriteHomingParameters.xError THEN
			_ClearAllExecute();
			_eLastMotionError := _fbMC_WriteHomingParameters.iErrorID;
			Raise(iErrorID  := enumAxisControl_ErrorID.HOMING_WRITE_PARAM_FAIL,
			      iNextStep := enumAxisControl_Step.READY);
			RETURN;
		ELSIF NOT _xHomeRequested_Prev THEN
			_ClearAllExecute();
			iStep := enumAxisControl_Step.STOPPING;
		END_IF

	// ── HOMING_EXEC：執行回原點動作 ───────────────────────────────────
	enumAxisControl_Step.HOMING_EXEC:
		xBusy := TRUE;
		_fbMC_Home.Position := stParameter.stHoming.lrHomingPositionOffset;
		_fbMC_Home.Execute  := TRUE;
		IF _fbMC_Home.Done THEN
			_xCommandDone   := TRUE;
			_iActiveCommand := enumAxisControl_Step.HOMING_EXEC;
			_ClearAllExecute();
			iStep := enumAxisControl_Step.STOPPING;
		ELSIF _fbMC_Home.Error THEN
			_ClearAllExecute();
			_eLastMotionError := _fbMC_Home.ErrorID;
			Raise(iErrorID  := enumAxisControl_ErrorID.HOMING_EXEC_FAIL,
			      iNextStep := enumAxisControl_Step.READY);
			RETURN;
		ELSIF NOT _xHomeRequested_Prev THEN
			_ClearAllExecute();
			iStep := enumAxisControl_Step.STOPPING;
		END_IF

	// ── SET_POSITION：設定座標（不含運動）───────────────────────────
	enumAxisControl_Step.SET_POSITION:
		xBusy := TRUE;
		// DoSetPosition 為 Abstract，由子類實作（管理 Execute 邊緣邏輯）
		_iDoSetPosResult := DoSetPosition(lrPosition := _lrSetPosTarget);
		IF _iDoSetPosResult = 1 THEN
			_xCommandDone   := TRUE;
			_iActiveCommand := enumAxisControl_Step.SET_POSITION;
			// SetPosition 不涉及運動，完成後直接回 READY
			iStep := enumAxisControl_Step.READY;
		ELSIF _iDoSetPosResult = -1 THEN
			Raise(iErrorID  := enumAxisControl_ErrorID.SET_POSITION_FAIL,
			      iNextStep := enumAxisControl_Step.READY);
			RETURN;
		END_IF

	// ── STOPPING：觸發 MC_Stop，立即進入 WAIT_STOP ───────────────────
	enumAxisControl_Step.STOPPING:
		xBusy := TRUE;
		IF NOT xEMS THEN
			// EMS 時改用 MC_Stop_EMS，不重複啟動 MC_Stop
			_fbMC_Stop.Deceleration := stParameter.lrStopDec;
			_fbMC_Stop.Jerk         := stParameter.lrStopJerk;
			_fbMC_Stop.Execute      := TRUE;
		END_IF
		iStep := enumAxisControl_Step.WAIT_STOP;

	// ── WAIT_STOP：等待停止完成 ────────────────────────────────────────
	enumAxisControl_Step.WAIT_STOP:
		xBusy := TRUE;
		IF xEMS THEN
			// EMS 時等待軸實際靜止（由 MC_Stop_EMS 控制）
			IF Axis.nAxisState=SM3_Basic.SMC_AXIS_STATE.standstill AND NOT xStop THEN
				iStep := enumAxisControl_Step.READY;
			END_IF
		ELSE
			IF _fbMC_Stop.Done AND NOT xStop THEN
				_xCommandDone   := TRUE;
				_iActiveCommand := enumAxisControl_Step.STOPPING;
				_fbMC_Stop.Execute := FALSE;
				iStep := enumAxisControl_Step.READY;
			END_IF
			IF _fbMC_Stop.Error THEN
				_fbMC_Stop.Execute := FALSE;
				_eLastMotionError := _fbMC_Stop.ErrorID;
				Raise(iErrorID  := enumAxisControl_ErrorID.DRIVER_ERROR,
				      iNextStep := enumAxisControl_Step.TRY_RESET);
				RETURN;
			END_IF
		END_IF

	// ── TRY_RESET：驅動器重置後恢復 ────────────────────────────────
	enumAxisControl_Step.TRY_RESET:
		xBusy := TRUE;
		_fbMC_Reset.Execute := TRUE;
		IF _fbMC_Reset.Done OR _fbMC_Reset.ErrorID=SM3_Error.SMC_R_NO_ERROR_TO_RESET THEN
			_fbMC_Reset.Execute := FALSE;
			IF Axis.nAxisState=SM3_Basic.SMC_AXIS_STATE.standstill AND NOT Axis.bError THEN
				iStep := _iStepBeforeReset;
			END_IF
		END_IF
		IF _fbMC_Reset.Error THEN
			_fbMC_Reset.Execute := FALSE;
			// 重置失敗：停留在 TRY_RESET 等待外部干預（xReset）
		END_IF

END_CASE