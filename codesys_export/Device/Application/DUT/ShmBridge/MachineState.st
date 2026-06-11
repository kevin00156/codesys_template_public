{attribute 'pack_mode' := '8'}
TYPE MachineState :
STRUCT
    axes      : ARRAY[0..3] OF AxisState;  (* 4 軸狀態，各 48 bytes — offset 0   *)
    uRunState : UDINT;                      (* 機台執行狀態            — offset 192 *)
    uAlarms   : UDINT;                      (* 機台警報 bitmask         — offset 196 *)
END_STRUCT
END_TYPE