TYPE enumAxisControl_ErrorID :
(
	NONE                    := 0,
	DRIVER_ERROR            := 100, // 驅動器回報錯誤（iErrorID 會同時包含驅動器錯誤碼）
	HOMING_WRITE_PARAM_FAIL := 201, // 寫入回原點參數失敗
	HOMING_EXEC_FAIL        := 202, // 執行回原點失敗
	MOVE_ABS_FAIL           := 301, // 絕對定位失敗
	MOVE_REL_FAIL           := 302, // 相對定位失敗
	MOVE_VEL_FAIL           := 303, // 速度模式失敗
	SET_POSITION_FAIL       := 401, // 設定位置失敗
	POSITIVE_LIMIT_TRIP     := 501, // 正極限觸碰
	NEGATIVE_LIMIT_TRIP     := 502, // 負極限觸碰
	DUMMY					:= 999	// 假訊號（由外部觸發）
) INT;
END_TYPE