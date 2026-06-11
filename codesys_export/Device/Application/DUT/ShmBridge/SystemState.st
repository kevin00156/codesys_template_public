{attribute 'pack_mode' := '8'}
TYPE SystemState :
STRUCT
    rTemperature : LREAL;   (* 系統溫度 °C          — offset 0  *)
    uStatusFlags : UDINT;   (* 狀態旗標 bitmask      — offset 8  *)
    uAlarmFlags  : UDINT;   (* 警報旗標 bitmask      — offset 12 *)
END_STRUCT
END_TYPE