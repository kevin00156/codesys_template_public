{attribute 'pack_mode' := '8'}
TYPE PlcData :
STRUCT
    header     : ShmHeader;     (* offset 0   — 24 bytes *)
    system     : SystemState;   (* offset 24  — 16 bytes *)
    machine    : MachineState;  (* offset 40  — 200 bytes *)
    production : ProductionState; (* offset 240 — 8 bytes *)
END_STRUCT
END_TYPE