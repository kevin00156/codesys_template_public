METHOD ReadDriverInputs
VAR
	_wFilteredCode : WORD;
END_VAR

// === IMPLEMENTATION ===
// 過濾 X57s 已知的正常狀態碼（非真實錯誤）
_wFilteredCode := wDriverErrorCode;
IF _wFilteredCode=21571 THEN//正極限
	_wFilteredCode:=0;
END_IF
IF _wFilteredCode=21572 THEN//負極限
	_wFilteredCode:=0;
END_IF

_wDriverErrorCode := _wFilteredCode;

// wDriverInputWord 位元映射（X57s PDO 定義）
_xPositiveLimitInput := wDriverInputWord.0;
_xNegativeLimitInput := wDriverInputWord.1;
_xBrakeInput         := wDriverInputWord.2;

// 更新輸出旗標
xPositiveLimitReached := _xPositiveLimitInput;
xNegativeLimitReached := _xNegativeLimitInput;