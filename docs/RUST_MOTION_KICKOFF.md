# Rust Motion Daemon — Kickoff 規格

本文件是 `feat/rust-motion` 分支的實作規格，內容來自 CODESYS → Rust 替換評估的結論。
新 session 開工前先完整讀完本文件、`docs/DEVELOPMENT.md` §1–§2、
`backend/internal/shm/`（layout.go / writer.go / reader.go / vet.go / seqlock_test.go）。

## 0. 背景與既定決策（不要重新評估，直接採用）

本 repo 是 **CODESYS PLC + Go 橋接器 + Svelte HMI** 範本。PLC 與 Go 只透過兩塊
shm（`/dev/shm/plc_data` PLC→Go、`/dev/shm/plc_cmd` Go→PLC）以 seqlock 協議溝通。
長期目標是以 **Rust 運動控制 daemon 取代 CODESYS**（動機：SoftMotion 軸授權成本、
IDE 體驗、記憶體安全），已定案的技術選型：

| 層 | 選擇 | 備註 |
|---|---|---|
| EtherCAT master | **ethercrab**（純 Rust） | 退路 IgH — 因此 ethercrab 型別不得洩漏出 adapter crate |
| 軌跡生成 | **rsruckig**（Ruckig port，jerk-limited） | 對應 MC_MoveAbsolute/MoveVelocity + Acc/Dec/Jerk 語意 |
| CiA 402 / homing | **自己 own**（statusword/controlword 層） | ethercrab 的 DS402 API 只能當便利包在自家 trait 後面用 |
| RT 模型 | smol executor + 絕對時間定時（`Timer::at`）+ SCHED_FIFO + mlockall | **不用 tokio**（jitter 實測差） |
| IPC | 沿用現有 shm seqlock 契約，**byte-identical、不 bump Version** | Go 與前端零改動 |

## 1. 本次實作目標

**部署後，前端（既有 Svelte HMI）能對軸下指令（power/jog/move/stop）並看到即時狀態。**

資料路徑（唯一改變的是最左邊的 writer 從 CODESYS 換成 Rust）：

```
Svelte HMI ── WS ──> Go plc_bridge ──> /dev/shm/plc_cmd ──> Rust motion-daemon
Svelte HMI <── WS ── Go plc_bridge <── /dev/shm/plc_data <── (fieldbus backend)
```

第一個真實 backend 用 **ethercrab 實作 EtherCAT PDO 循環**（本次主要實作目標）。
同時提供 **sim backend**（純軟體軸模型），讓無硬體環境（WSL / CI）能 end-to-end 驗證。

## 2. 關鍵架構要求：fieldbus 抽象層

**軸控不一定走 EtherCAT PDO。** 低成本、非即時的設備可能用 Modbus TCP/RTU 或其他
通訊手段。因此運動核心與匯流排之間必須有 trait seam：

- `fieldbus-api` trait 至少要表達：
  - **cyclic process image**：每軸的輸入/輸出映像交換（EtherCAT = 每週期 PDO；
    Modbus = 低頻輪詢暫存器）
  - **acyclic 通道**：SDO / 參數暫存器讀寫（設定、診斷用，不在 RT 路徑上）
  - **bus 狀態事件**：INIT/PREOP/SAFEOP/OP、斷線、slave 錯誤（Modbus 對應
    connected/timeout 等簡化狀態）
  - **capability 描述**（per-axis / per-backend）：
    - 週期等級：hard-RT（1ms, DC 同步）vs soft（10–100ms 輪詢）
    - setpoint 語意：**每週期軌跡點**（CSP，軌跡由 motion-core 的 rsruckig 生成）
      vs **目標值轉發**（位置/速度暫存器寫入，軌跡在 drive/變頻器內部跑）
    - 支援的操作模式（position / velocity / torque / homing）
- `motion-core` 依 capability 決定：自己跑 rsruckig 逐週期出點，或只把目標值 +
  限制參數轉發給 drive。
- **本次不實作 fieldbus-modbus**，但 trait 設計必須先容納上述差異
  （寫 trait 時用一個假想的 Modbus 變頻器軸當 design test 想一遍）。

## 3. 建議 workspace 結構（`rust/`）

```
rust/
├── Cargo.toml                 # workspace
├── README.md                  # 架構決策記錄（ADR 式，含 trait 設計理由）
├── motion-core/               # 軸狀態機 + 軌跡。不依賴任何匯流排型別
├── fieldbus-api/              # trait 定義（上面 §2）
├── fieldbus-ethercat/         # ethercrab adapter（ethercrab 型別只活在這裡）
├── fieldbus-sim/              # 模擬 backend（一階/二階軸模型即可）
├── shm-bridge/                # layout 復刻 + seqlock writer/reader
└── motion-daemon/             # binary：設定檔、thread 佈局、組裝
```

## 4. 硬性約束

1. **shm 契約 byte-identical**，single source of truth 是
   [backend/internal/shm/layout.go](../backend/internal/shm/layout.go)：
   - `plc_data`：248 bytes、Magic `0x504C4344`、Version **4**
   - `plc_cmd`：168 bytes、Magic `0x504C4343`、Version **3**
   - Header 24B；`MachineState` 4 軸 × `AxisState` 48B；`MachineCmd` 4 軸 × `AxisCmd` 32B
   - Rust 端用 `#[repr(C)]` + `const` size assert 復刻（對應 Go 端 vet.go 的模式），
     並寫「與 Go 測試向量對拍」的測試（同一組 bytes 兩邊解出相同欄位值）
   - seqlock 協議照 `writer.go`/`reader.go`：寫入期間 Seq 為奇數，reader 見奇數或
     前後不一致即重讀。Rust daemon 是 `plc_data` 的 **writer**、`plc_cmd` 的 **reader**
   - segment 由 daemon 建立（原本是 CODESYS 建的），名稱/大小/權限比照現況
     （`plc_bridge.service.unit` 的 ExecStartPre 會等 segment 出現並放寬權限）
2. **軸狀態機語意照 `MC_BasicControl`**（`codesys_export/.../MC_BasicControl/`）：
   power/jog±/moveAbs/moveRel/moveVel/home/stop/EMS + 請求旗標模式。
   `AxisState.Step` 發布的數值必須與
   `codesys_export/.../Structure/enumAxisControl_Step.st` 的 enum 值一致（HMI 靠它顯示）；
   `ErrorID`、`Flags` bitmask 同理照 layout.go 註解。
3. **RT 紀律**（EtherCAT backend 的 cycle thread）：SCHED_FIFO、`mlockall`、
   cycle 內零 heap allocation、絕對時間定時。sim backend 不需 RT，一般 thread 即可。
4. **跨平台編譯**：比照 Go 端 `mapping_linux.go` / `mapping_other.go` 的模式 —
   Linux-only 的東西（shm mmap、RT 排程、raw socket）用 `#[cfg(target_os = "linux")]`
   隔離 + 非 Linux stub，`cargo build`/`cargo test`（motion-core、fieldbus-api、sim、
   狀態機測試）必須能在 Windows dev box 與 CI 上過。
5. **ethercrab 使用規範**：型別不得出現在 fieldbus-ethercat 以外的 crate；
   PDI guard 持有時間最短化（0.6+ 是 spinlock）；雙主站未來 = 兩個 MainDevice
   實例綁兩張 NIC，設計時不要寫死單主站假設。

## 5. 分階段與驗收

**Phase 1 — 骨架 + sim end-to-end**
workspace + `fieldbus-api` trait + `shm-bridge`（含 layout 對拍測試）+ `fieldbus-sim`
+ `motion-daemon` 骨架（設定檔指定 backend 與軸數）。
✅ 驗收：WSL 裡跑 `motion-daemon --backend sim` + 既有 `plc_bridge`，前端登入後可
enable/jog 模擬軸並看到位置變化（走 `make wsl-deploy` 流程，見
`.claude/skills/wsl-deploy-strategy`）。

**Phase 2 — motion-core**
狀態機（MC_BasicControl 語意）+ rsruckig 軌跡 + 單元測試（每個命令的
Step 轉移、EMS 打斷、錯誤路徑）。
✅ 驗收：`cargo test` 全過；sim backend 下 moveAbs 的位置曲線是 jerk-limited S-curve。

**Phase 3 — fieldbus-ethercat PDO（本次主要目標）**
ethercrab adapter：拓撲掃描 → PREOP 設定（PDO mapping、DC）→ OP → 1ms cyclic PDO
迴圈，接進 trait。CiA 402 電源狀態機（statusword/controlword）在這層或
motion-core 的 402 模組實作 — 依 trait 設計決定，寫進 rust/README.md。
✅ 驗收：能編譯 + `examples/` 有可跑的 bring-up 範例（掃描、進 OP、cyclic 交換、
jitter 統計輸出）。實機驗證留待有硬體的環境，但範例的參數（週期、NIC 名稱）
要可設定。

## 6. 已知風險 / 待實機驗證清單（先記錄，不阻塞）

- ethercrab DC 同步品質在多軸/雙主站下未驗證 — Phase 3 的 examples 要輸出
  jitter/DC 漂移統計，方便日後上機直接量
- 實際 drive（如 LC10E）的 homing 模式與怪癖 — 對應舊 ST 碼
  `MC_WriteHomingParameters`、`MC_ActualDriver_SetPosition` 的領域知識要搬過來
- shm 權限：daemon 以何 user 跑、segment 權限與 `wsl-bootstrap.sh` 的
  ExecStartPre 配合 — Phase 1 在 WSL 驗
- rsruckig 輸出品質 — 之後與 C++ 原版 Ruckig 對拍
