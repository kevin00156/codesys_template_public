{attribute 'pack_mode' := '8'}
TYPE AxisCmd :
STRUCT
    uControlFlags : UDINT;   (* bit0=Enable bit1=Home bit2=Reset
                                bit3=Stop  bit4=JogPos bit5=JogNeg
                                bit6=MoveAbs               — offset 0 *)
    _reserved     : UDINT;   (* 保留                        — offset 4 *)
    lrJogVel      : LREAL;   (* Jog 速度                    — offset 8 *)
    lrMoveAbsPos  : LREAL;   (* MoveAbsolute 目標位置        — offset 16 *)
    lrMoveAbsVel  : LREAL;   (* MoveAbsolute 速度            — offset 24 *)
END_STRUCT
END_TYPE