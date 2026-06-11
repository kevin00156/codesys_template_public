TYPE structAxisControl_MotionProfile :
STRUCT
	lrVelocity	: LREAL := 10;		// 速度（所有命令共用預設值）
	lrAcc		: LREAL := 100;		// 加速度
	lrDec		: LREAL := 100;		// 減速度
	lrJerk		: LREAL := 1000;	// 加加速度
END_STRUCT
END_TYPE