TYPE structAxisControl_Parameter :
STRUCT
	// 所有運動命令共用的預設速度輪廓
	// 當 method 呼叫時傳入 0，則使用此處的值
	stDefault				: structAxisControl_MotionProfile;

	// 停止參數
	lrStopDec				: LREAL := 100;		// 一般停止減速度
	lrStopJerk				: LREAL := 1000;	// 一般停止加加速度
	lrEmergencyStopDec		: LREAL := 1000;	// EMS 立即停止減速度

	// 回原點參數
	stHoming				: structAxisControl_HomingProfile;

	// 功能旗標
	xContinuousUpdate		: BOOL;				// TRUE=Jog/Velocity 速度即時更新
END_STRUCT
END_TYPE