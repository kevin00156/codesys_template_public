FUNCTION_BLOCK ABSTRACT MC_BasicControl EXTENDS BaseStateMachine
VAR_IN_OUT
	Axis        : AXIS_REF_SM3;
	stParameter : structAxisControl_Parameter;
END_VAR
VAR_INPUT
	xEMS         : BOOL;	// 緊急停止（立即，不等減速）
	xJogPositive : BOOL;	// Jog 正方向（跨任務持久訊號，HMI 用）
	xJogNegative : BOOL;	// Jog 負方向（跨任務持久訊號，HMI 用）
END_VAR
VAR_OUTPUT
	xAxisStandStill       : BOOL;	// 軸停止不動
	xPositiveLimitReached : BOOL;	// 正向極限觸動
	xNegativeLimitReached : BOOL;	// 負向極限觸動
	xInVelocity           : BOOL;	// 速度命令中（已達目標速度）
END_VAR
VAR
	// 電源控制旗標（由狀態機與 Stop() 管理）
	_xPowerOn : BOOL;

	// 驅動器 IO 旗標（由 ReadDriverInputs 填入）
	_xPositiveLimitInput : BOOL;
	_xNegativeLimitInput : BOOL;
	_xBrakeInput         : BOOL;
	_wDriverErrorCode    : WORD;

	// 命令請求旗標（當前掃描，由 method 設定）
	_xJogPosRequested  : BOOL;
	_xJogNegRequested  : BOOL;
	_xMoveAbsRequested : BOOL;
	_xMoveRelRequested : BOOL;
	_xMoveVelRequested : BOOL;
	_xHomeRequested    : BOOL;
	_xSetPosRequested  : BOOL;

	// 命令請求旗標（前一掃描，供狀態機讀取）
	_xJogPosRequested_Prev  : BOOL;
	_xJogNegRequested_Prev  : BOOL;
	_xMoveAbsRequested_Prev : BOOL;
	_xMoveRelRequested_Prev : BOOL;
	_xMoveVelRequested_Prev : BOOL;
	_xHomeRequested_Prev    : BOOL;
	_xSetPosRequested_Prev  : BOOL;

	// 命令參數（由 method 解析後填入）
	_lrJogVelocity : LREAL;
	_lrJogAcc      : LREAL;
	_lrJogDec      : LREAL;
	_lrJogJerk     : LREAL;

	_lrSetPosTarget : LREAL;

	// 命令完成追蹤
	_xCommandDone      : BOOL;
	_iActiveCommand    : INT;					// 最近完成的命令對應的 enumAxisControl_Step
	_eLastMotionError  : SM3_Error.SMC_ERROR;	// 最近一次 SM3 動作錯誤碼（供 method 回傳）

	// MoveRelative 重新觸發鎖：Done 後設 TRUE，呼叫端停止呼叫後清除
	// 防止持續呼叫 MoveRelative() 時完成後立刻開始下一次相對移動
	_xMoveRelNeedsReArm : BOOL;

	// 重置後返回步驟
	_iStepBeforeReset : INT;

	// SM3 Motion FBs
	_fbMC_Power                 : MC_Power;
	_fbMC_Reset                 : MC_Reset;
	_fbMC_MoveAbsolute          : MC_MoveAbsolute;
	_fbMC_MoveRelative          : MC_MoveRelative;
	_fbMC_MoveVelocity          : MC_MoveVelocity;
	_fbMC_Stop                  : MC_Stop;
	_fbMC_Stop_EMS              : MC_Stop;
	_fbMC_Home                  : MC_Home;
	_fbMC_WriteHomingParameters : MC_WriteHomingParameters;

	// Jog 速度變化偵測（ContinuousUpdate 邏輯）
	_fbVCD_Jog : ValueChangeDetector;
END_VAR

// === IMPLEMENTATION ===
xEnabled        := Axis.bRegulatorOn;
SUPER^();
_RunMotionFBs();