{attribute 'pack_mode' := '8'}
TYPE ProductionState :
STRUCT
    nProductionState : DINT;   (* 生產狀態編號 *)
    _reserved        : DINT;   (* 保留，供未來擴充 *)
END_STRUCT
END_TYPE