{attribute 'pack_mode' := '8'}
TYPE MachineCmd :
STRUCT
    axes          : ARRAY[0..3] OF AxisCmd;  (* 各軸指令，各 32 bytes  — offset 0   *)
    uControlFlags : UDINT;                    (* bit0=Reset bit1=EMS
                                                bit2=SystemRun          — offset 128 *)
    _reserved     : UDINT;                    (* 保留                    — offset 132 *)
END_STRUCT
END_TYPE