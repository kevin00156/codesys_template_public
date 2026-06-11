{attribute 'pack_mode' := '8'}
TYPE ShmHeader :
STRUCT
    magic   : UDINT;   (* offset 0  : segment-specific magic *)
    version : UINT;    (* offset 4  : layout version *)
    flags   : UINT;    (* offset 6  : reserved *)
    seq     : UDINT;   (* offset 8  : seqlock; even=stable, odd=writing *)
    _pad    : UDINT;   (* offset 12 : keep cycle 8-byte aligned *)
    cycle   : ULINT;   (* offset 16 : producer cycle counter *)
END_STRUCT
END_TYPE