TYPE structAxisControl_HomingProfile :
STRUCT
	lrUnitToPulseRatio				: LREAL;	// 單位/脈衝比率（驅動器參數用）
	lrHoming_Acceleration			: LREAL;	// 回原點加速度
	lrVelocityDuringSearchForZero	: LREAL;	// 尋零速度（快速段）
	lrVelocityDuringSearchForSwitch	: LREAL;	// 找開關速度（慢速段）
	lrHomingPositionOffset			: LREAL;	// 原點位置偏移（完成後的座標設定值）
	uiHomingMethod					: UINT;		// 回原點模式編號（驅動器定義）
END_STRUCT
END_TYPE