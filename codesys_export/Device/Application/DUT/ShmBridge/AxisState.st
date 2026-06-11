{attribute 'pack_mode' := '8'}
TYPE AxisState :
STRUCT
    lrActPos  : LREAL;   (* 實際位置 mm/user-unit  — offset 0  *)
    lrActVel  : LREAL;   (* 實際速度 mm/s          — offset 8  *)
    lrSetPos  : LREAL;   (* 命令位置               — offset 16 *)
    lrSetVel  : LREAL;   (* 命令速度               — offset 24 *)
    iStep     : DINT;    (* enumAxisControl_Step    — offset 32 *)
    uFlags    : UDINT;   (* bit0=Enabled bit1=Busy bit2=Error
                            bit3=StandStill bit4=PosLimit bit5=NegLimit
                                                   — offset 36 *)
    iErrorID  : DINT;    (* SMC_ERROR               — offset 40 *)
    _reserved : DINT;    (* 保留                    — offset 44 *)
END_STRUCT
END_TYPE