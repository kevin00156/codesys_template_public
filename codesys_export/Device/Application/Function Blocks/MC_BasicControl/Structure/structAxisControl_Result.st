TYPE structAxisControl_Result :
STRUCT
	xBusy		: BOOL;						// 命令執行中
	xDone		: BOOL;						// 命令完成（持續一個掃描週期）
	xError		: BOOL;						// FB 錯誤中
	eErrorID	: SM3_Error.SMC_ERROR;		// SM3 原始錯誤碼
END_STRUCT
END_TYPE