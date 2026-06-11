METHOD ABSTRACT ReadDriverInputs
// 由子類（MC_BasicControl_<DriverName>）實作
// 必須填入以下保護欄位：
//   _xPositiveLimitInput : BOOL
//   _xNegativeLimitInput : BOOL
//   _xBrakeInput         : BOOL
//   _wDriverErrorCode    : WORD（若無錯誤則為 0，驅動器特定碼在此過濾）
// 並更新輸出：
//   xPositiveLimitReached := _xPositiveLimitInput
//   xNegativeLimitReached := _xNegativeLimitInput

// === IMPLEMENTATION ===