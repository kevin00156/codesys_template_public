TYPE enumAxisControl_Step :
(
	IDLE                := 0,   // 閒置/關電狀態（BaseStateMachine Stop() 後的起點）
	ENABLING            := 10,  // 等待驅動器激磁
	READY               := 20,  // 就緒，等待命令
	NOT_READY           := 25,  // 軸狀態異常，等待恢復
	JOG_POS             := 30,  // 正向 JOG 執行中
	JOG_NEG             := 31,  // 反向 JOG 執行中
	MOVE_ABS            := 40,  // 絕對定位執行中
	MOVE_REL            := 50,  // 相對定位執行中
	MOVE_VEL            := 60,  // 速度模式執行中
	HOMING_WRITE_PARAM  := 70,  // 寫入回原點參數中
	HOMING_EXEC         := 71,  // 執行回原點中
	SET_POSITION        := 80,  // 設定位置執行中
	STOPPING            := 90,  // MC_Stop 命令發出中
	WAIT_STOP           := 91,  // 等待 MC_Stop 完成
	TRY_RESET           := 95   // 嘗試重置伺服錯誤
) INT;
END_TYPE