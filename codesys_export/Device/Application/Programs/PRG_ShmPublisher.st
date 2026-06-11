PROGRAM PRG_ShmPublisher
VAR
    bInit   : BOOL;
    bFault  : BOOL;

    hData     : RTS_IEC_HANDLE;
    hCmd      : RTS_IEC_HANDLE;
    pData     : POINTER TO PlcData;
    pCmd      : POINTER TO PlcCommand;
    udiResult : RTS_IEC_RESULT;
    uDataSize : __UXINT;
    uCmdSize  : __UXINT;

    sDataName : STRING := 'plc_data';
    sCmdName  : STRING := 'plc_cmd';

    cmdLast       : PlcCommand;
    bCmdValid     : BOOL;
    uLastCmdCycle : ULINT := 0;

    (* HMI 指令的當前狀態 — Watch 可直接改，HMI 送新指令時整體更新 *)
    currentCmd : PlcCommand;
END_VAR
VAR_TEMP
    s1, s2 : UDINT;
    snap   : PlcCommand;
    i      : INT;
END_VAR

// === IMPLEMENTATION ===
(* === INIT (run once) === *)
IF NOT bInit THEN
    uDataSize := SIZEOF(PlcData);
	
    hData := SysSharedMemoryCreate(sDataName, 0, ADR(uDataSize), ADR(udiResult));
    IF hData = RTS_INVALID_HANDLE THEN bFault := TRUE; RETURN; END_IF
    pData := SysSharedMemoryGetPointer(hData, ADR(udiResult));
    IF pData = 0 THEN bFault := TRUE; RETURN; END_IF

    uCmdSize := SIZEOF(PlcCommand);
    hCmd := SysSharedMemoryCreate(sCmdName, 0, ADR(uCmdSize), ADR(udiResult));
    IF hCmd = RTS_INVALID_HANDLE THEN bFault := TRUE; RETURN; END_IF
    pCmd := SysSharedMemoryGetPointer(hCmd, ADR(udiResult));
    IF pCmd = 0 THEN bFault := TRUE; RETURN; END_IF

    pData^.header.magic   := PLC_DATA_MAGIC;
    pData^.header.version := PLC_DATA_VERSION;
    pData^.header.seq     := 0;

    bInit := TRUE;
END_IF

IF bFault THEN RETURN; END_IF


(* === Read command — bounded retry seqlock === *)
FOR i := 0 TO 99 DO
    s1 := pCmd^.header.seq;
    IF (s1 MOD 2) = 0 THEN
        snap := pCmd^;
        s2 := pCmd^.header.seq;
        IF s1 = s2
           AND snap.header.magic   = PLC_CMD_MAGIC
           AND snap.header.version = PLC_CMD_VERSION
        THEN
            cmdLast   := snap;
            bCmdValid := TRUE;
            EXIT;
        END_IF
    END_IF
END_FOR

(* 新指令到達時整體套用 — 加新欄位不需修改此處 *)
IF bCmdValid AND (cmdLast.header.cycle <> uLastCmdCycle) THEN
    uLastCmdCycle := cmdLast.header.cycle;
    currentCmd    := cmdLast;
END_IF

GVL.plcCommand REF= currentCmd;

(* === Publish via seqlock === *)
pData^.header.seq := pData^.header.seq + 1;

(* 測試用：改為 GVL.* 接上真實專案
   pData^.system  := GVL.system;
   pData^.machine := GVL.machine;                          *)
GVL.plcData REF= pData^;

pData^.header.cycle := pData^.header.cycle + 1;
pData^.header.seq   := pData^.header.seq + 1;