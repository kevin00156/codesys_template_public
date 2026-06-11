METHOD ABSTRACT DoSetPosition : INT
VAR_INPUT
	lrPosition : LREAL;	// 目標位置
END_VAR
// 回傳值：
//   0 = 執行中（Busy）
//   1 = 完成（Done）
//  -1 = 錯誤（Error）
//
// 實作說明：子類需管理 Execute 邊緣偵測（上升沿觸發）
// 建議：使用內部旗標在首次呼叫時送出 Reset 脈衝

// === IMPLEMENTATION ===