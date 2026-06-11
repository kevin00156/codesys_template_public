{attribute 'pack_mode' := '8'}
TYPE PlcCommand :
STRUCT
    header     : ShmHeader;      (* offset 0   — 24 bytes *)
    machine    : MachineCmd;     (* offset 24  — 136 bytes *)
    production : ProductionState; (* offset 160 — 8 bytes *)
END_STRUCT
END_TYPE