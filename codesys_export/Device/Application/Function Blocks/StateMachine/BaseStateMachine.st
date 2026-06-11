FUNCTION_BLOCK ABSTRACT BaseStateMachine IMPLEMENTS IStateMachine
VAR_INPUT
	xEnable : BOOL;
	xExecute : BOOL;
	xReset : BOOL;
	xStop : BOOL;
	xPause : BOOL;
END_VAR
VAR_OUTPUT
	xEnabled 			: BOOL;
	xBusy 				: BOOL;
	xDone 				: BOOL;
	iStep       		: INT;      // 當前步驟
    iErrorID    		: INT;      // 錯誤代碼
    xError      		: BOOL;     // 錯誤中
    xPaused     		: BOOL;     // 暫停中
END_VAR
VAR
	iLastStep   		: INT;      // 用於暫停後恢復
	iNextStep			: INT;		// 用於Step間停等的暫存
	_iRaiseStep 		: INT;		// 用於監視上次出錯的Step，沒有實際用途
	_xInitialized 		: BOOL;		// FB初始動作
END_VAR

// === IMPLEMENTATION ===
//初始處理
IF NOT _xInitialized THEN
	IF Init() THEN
		_xInitialized:=TRUE;
	END_IF
END_IF

// 錯誤處理
IF xReset THEN
	Reset();
END_IF
IF xError THEN
	IF xStop THEN
		Stop();
	END_IF
    RETURN;
END_IF

//暫停處理
IF xPause THEN
	Pause();
END_IF
IF xPaused THEN
	IF NOT xPause THEN
		Resume();
	END_IF
    RETURN;
END_IF

//ENABLE處理
IF NOT xEnable THEN
	Stop();
	RETURN;
END_IF
IF xStop THEN
	Stop();
END_IF

//主邏輯
Main();