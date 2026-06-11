FUNCTION LMOD : LREAL
// LREAL 取模：將 lrValue 規範化到 [0, lrModulus) 區間
// CODESYS 標準 MOD 運算子不支援 LREAL，須改用 FLOOR 公式
//
// 用法：lrAngle := LMOD(lrAngle, 360.0);
// 對負值與週期外的值都能正確處理

VAR_INPUT
	lrValue   : LREAL;
	lrModulus : LREAL;
END_VAR

// === IMPLEMENTATION ===
IF lrModulus <= 0.0 THEN
	LMOD := 0.0;
	RETURN;
END_IF

LMOD := lrValue - FLOOR(lrValue / lrModulus) * lrModulus;
IF LMOD < 0.0 THEN
	LMOD := LMOD + lrModulus;
END_IF