{attribute 'pack_mode' := '8'}
TYPE MotionState :
STRUCT
    axisPos   : LREAL;   (* offset 0  *)
    axisVel   : LREAL;   (* offset 8  *)
    axisFlags : UDINT;   (* offset 16 : bit0=enabled bit1=homed bit2=moving *)
    _pad      : UDINT;   (* offset 20 *)
END_STRUCT
END_TYPE