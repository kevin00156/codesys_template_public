# rust/ — Motion Daemon Workspace

CODESYS 替換案的 Rust 端（規格見 [docs/RUST_MOTION_KICKOFF.md](../docs/RUST_MOTION_KICKOFF.md)）。
本檔記錄 **trait 與架構設計決策**（ADR 式）：每一條都是有意識的取捨，改動前先讀理由。

```
rust/
├── fieldbus-api/        # trait seam：cyclic image / acyclic / events / capability
├── fieldbus-ethercat/   # ethercrab adapter（ethercrab 型別只活在這裡；含 bring-up example）
├── fieldbus-sim/        # 軟體軸模型（WSL / CI 無硬體 E2E）
├── motion-core/         # MC_BasicControl 語意狀態機 + 軌跡（不認識任何匯流排型別）
├── shm-bridge/          # layout.go 的 byte-identical 復刻 + seqlock + segment 建立
└── motion-daemon/       # binary：設定、組裝、cycle loop
```

依賴方向（單向，違反即架構錯誤）：

```
motion-daemon ──> motion-core ──> fieldbus-api <── fieldbus-{sim,ethercat,modbus…}
      └─────────> shm-bridge（只有 daemon 碰 IPC）
```

- `motion-core` 只依賴 `fieldbus-api`。
- `shm-bridge` 誰都不依賴；只有 `motion-daemon` 同時看得到 shm 與 fieldbus。
- adapter crate 之間互不認識。

## 建置 / 測試

```
cd rust
cargo build --workspace     # Windows / Linux 都必須過（Linux-only 部分 cfg 隔離）
cargo test  --workspace
```

跨語言 layout 對拍：fixtures 在 `backend/internal/shm/testdata/*.bin`，由 Go 端
`golden_test.go` 產生（`go test ./backend/internal/shm -run Golden -update`，
只在**蓄意改 layout + bump Version** 時重生），Rust 端 `shm-bridge/tests/go_parity.rs`
解碼比對每個欄位並回編碼驗 byte 相等。

---

## ADR-1: fieldbus trait 邊界放在「正規化軸映像」，不是原始 PDO

**決策**：`Fieldbus::exchange(outs: &[AxisOut], ins: &mut [AxisIn])` 交換的是
bus 無關的正規化影像（位置/速度為 f64 使用者單位、`DriveStatus` 枚舉、
`Setpoint` 枚舉），不是 statusword/controlword 或暫存器原始值。

**理由**：軸控不一定走 EtherCAT。Modbus 變頻器沒有 CiA 402 可言，若 trait 傳
402 字組，sim 與 Modbus adapter 都得「假裝」402；反向讓 adapter 負責翻譯到
正規化語意，motion-core 完全 profile 無關。增量↔使用者單位換算也是 adapter
的事（每軸齒比/導程屬於 bus 設定）。

**Design test（假想 Modbus 變頻器軸）**：run bit ↔ `AxisOut::enable`、
fault register ↔ `fault_code` + `DriveStatus::Fault`、目標頻率暫存器 ↔
`Setpoint::TargetVelocity`、connected/timeout ↔ `BusState::Init/Op` +
`AxisOffline` 事件 — 全部裝得下，無一需要改 trait。

## ADR-2: trait 是 async（AFIT），只做 static dispatch

**決策**：`Fieldbus`/`AcyclicAccess` 用 `async fn` in trait；daemon 對每個
backend 單態化 cycle loop（`run::<SimBackend>` / `run::<EcatBackend>`），
啟動時 enum match 選擇，不用 `dyn`。

**理由**：既定 RT 模型是 smol executor + `Timer::at` 絕對時間定時，而
ethercrab 的 `tx_rx` 本身是 future；trait 若是 sync，adapter 內只能再包一層
`block_on`，executor 疊 executor。AFIT 缺 dyn-compatibility 沒關係——熱路徑
本來就不准 boxed future（每 cycle 一次 heap allocation 直接違反 RT 紀律）。
sim 的 future 恆為 immediately-ready，零開銷。

## ADR-3: capability 是資料，motion-core 據此選策略

**決策**：`AxisCapability { cycle: HardRt|SoftPoll, setpoint: CyclicTrajectory|TargetForwarding, modes }`
per-axis 查詢；`SetpointKind::CyclicTrajectory` ⇒ motion-core 跑軌跡（Phase 2 起
rsruckig）逐週期出 `CyclicPosition{pos, vel_ff}`；`TargetForwarding` ⇒ 只轉發
`TargetPosition/TargetVelocity{…, acc, dec}`，軌跡在 drive 內部跑。

**理由**：這正是 EtherCAT CSP 與 Modbus 變頻器的本質差異，寫成型別而不是
if-else 散在各處。**現狀**：Phase 1 只實作 CyclicTrajectory 策略（sim 刻意
宣告與 EtherCAT CSP 相同的 capability，讓 motion-core 走到 Phase 3 會走的同
一條路）；TargetForwarding 的 motion-core 分支等 fieldbus-modbus 一起做，
trait 已預留完整。

## ADR-4: CiA 402 電源狀態機住在 fieldbus-ethercat 裡，自己 own

**決策**：402 statusword 解碼 / controlword 走序（Shutdown→Switch On→Enable
Operation、fault reset 邊緣）實作在 `fieldbus-ethercat` 內部的 `cia402` 模組
（Phase 3），對外只呈現 `DriveStatus` + `AxisOut::enable/fault_reset`。
ethercrab 的 DS402 便利 API 至多內部參考，不進公開路徑。

**理由**：402 是 drive profile、不是 motion 語意——motion-core 只關心「要
使能／已使能／故障」。放 adapter 層讓 sim 不必模擬 statusword、Modbus 變頻器
不必假裝 402。若未來出現 402-over-Modbus 的伺服，`cia402` 模組再升格成獨立
crate 給兩個 adapter 共用（純狀態機、可單元測試，搬家成本低）。
`ParamAddr::Object{index,sub}` 已同時容納 CoE SDO 與 402-over-Modbus 位址。

## ADR-5: acyclic 通道是獨立 handle，不佔 cycle 路徑

**決策**：`Fieldbus::acyclic()` 回傳 adapter 自訂的 handle 型別
（`type Acyclic: AcyclicAccess`），在 service thread 上用；SDO / 參數暫存器
讀寫永不進 cycle loop。

**理由**：SDO 是毫秒級 mailbox 往返，放進 1ms cycle 是自殺；associated type
讓 ethercrab 的 SDO 管線藏在 adapter 內（型別不外漏），sim 用
`Arc<Mutex<BTreeMap>>` 就交差。homing 參數（`MC_WriteHomingParameters` 的
領域知識）將來走這條路寫進 drive。

## ADR-6: 事件用 `Copy` enum 輪詢，不用 channel/callback

**決策**：`fn poll_event(&mut self) -> Option<BusEvent>`，`BusEvent` 全 `Copy`
（`BusStateChanged/AxisOnline/AxisOffline/AxisFault`），cycle loop 內 drain。

**理由**：callback 進 RT 路徑會引入不可控的執行時機；channel 要配 executor 與
配額。單執行緒輪詢零配置、順序確定，Modbus 的 connected/timeout 對映
`BusState::Init/Op` + `AxisOffline` 即可。

## ADR-7: seqlock 的 Rust 實作 — 協議不變，記憶體模型補強

**決策**：wire 協議與 Go/IEC 完全相同（odd=寫入中、Seq 前後不一致重讀、
`SEQLOCK_MAX_RETRIES=100`），但 payload 拷貝用逐 `u64` relaxed atomic +
acquire/release fence（Boehm 式 fence-based seqlock），containing-Seq 的
word 由 `AtomicU32` 獨佔（避免 mixed-size atomic access）。

**理由**：Go 版的 bulk copy 在 C11 模型下是 data race，靠 x86-TSO + Go runtime
容忍；Rust 這樣寫是 UB。fence 版在 x86-64 編譯成純 MOV + compiler barrier，
**零成本、線上 byte 不變**，但在 ARM 等弱序平台也正確。兩個 segment 大小都是
8 的倍數（248/168），compile-time assert 釘住。

其他 shm 決策（照 kickoff §4）：
- segment 由 daemon `shm_unlink` + `O_CREAT|O_EXCL` **重建**（CODESYS 每次
  重啟也重建），開檔權限直接給足（plc_data 0644 / plc_cmd 0666），
  `plc_bridge.service.unit` 的 ExecStartPre chmod 變成冗餘但無害；退出時
  **不 unlink**（bridge 保留最後資料，與現狀一致）。
- `plc_cmd` header 不由 daemon 初始化（PRG_ShmPublisher 也沒有）：magic 無效
  就當「橋接器還沒上線」，杜絕撿到舊 segment 的殘留命令。
- 命令消費照 `PRG_ShmPublisher.st`：seqlock 快照 + **`header.cycle` 變化才
  整包 latch**；Go 端 `cmdSink` 每次接受命令 bump cycle。旗標因此是
  level-held——HMI 按住 jog = 舊 word 持續有效，放開送 word=0。

## ADR-8: MC_BasicControl 語意複刻與兩處記錄在案的偏差

`motion-core::axis` 照 `MC_BasicControl.Main.st` 逐節複刻：step 數值/
ErrorID/旗標 bitmask 與 IEC enum 完全一致（單元測試釘死）、READY 分派優先序
相同、EMS 為 level 且凍結狀態機並強制減速、跨過 IDLE 後 Enable 位元清除不會
斷電（Main.st 只在 IDLE 檢查 xEnable —— 這讓前端「Jog 按下送 16」不會踢掉
使能）。偏差：

1. **MOVE_ABS / HOME 有 re-arm latch**（Main.st 只有 MOVE_REL 有）：完成後
   同一個 latched word 不會重觸發，只有**新命令訊息**（`header.cycle` 變化）
   會 re-arm。原因：shm 命令是離散訊息而非每掃描呼叫；沒有 re-arm，latched
   MOVE_ABS 位元會讓 Step 永遠在 MOVE_ABS→STOPPING→READY 打轉（HMI 顯示閃爍）。
2. **錯誤恢復一律要求 HMI RESET 位元**（Main.st 的 TRY_RESET 部分路徑會
   自動重試）：極限觸發/驅動故障後停在 TRY_RESET 等 Reset 鍵。50–100ms HMI
   鏈路上顯式復位比自動恢復可預期，也安全。

Phase 2 帶 rsruckig 進來時（`profile.rs` 是唯一要換的模組），狀態機單元測試
會擴成全命令矩陣。

## ADR-9: 跨平台 — cfg 隔離照抄 Go 的 build-tag 模式

Linux-only（`/dev/shm` mmap、`signal`、`mlockall`/`SCHED_FIFO`、將來的 raw
socket）都在 `#[cfg(target_os = "linux")]` 後面，非 Linux 給會編譯的 stub
（`mapping_other` 對應 Go 的 `mapping_other.go`）。`cargo build/test` 在
Windows dev box 全綠是硬性驗收，CI 同。

## ADR-10: ethercrab 依賴只掛在 Linux target，非 Linux 走 stub backend

**決策**：`fieldbus-ethercat` 的 `ethercrab` 依賴宣告在
`[target.'cfg(target_os = "linux")'.dependencies]`；非 Linux 平台編譯
`backend_stub.rs`（同一公開介面，`start()` 回 `Unsupported`），純邏輯模組
（`cia402`、`pdo`、`config`）全平台編譯與測試。

**理由**：ethercrab 在 Windows/macOS 的 raw socket 後端需要 npcap/libpcap
SDK 才能連結，乾淨 Windows dev box 的 `cargo build` 會炸 —— 違反硬性驗收。
cfg 隔離（Go 端 `mapping_other.go` 的同一招）讓 Windows 綠、WSL 與 ubuntu CI
編譯真 adapter。副作用可接受：Windows 上跑不了 EtherCAT 本來就是事實。

## ADR-11: CSP PDO 佈局固定、controlword 慢一拍、enable 前 target 鎖 actual

**決策**（`pdo.rs` + `backend.rs`）：
- 我們主動把 PDO mapping 寫進 drive（0x1C12/0x1600、0x1C13/0x1A00）：
  RxPDO = controlword + target position（6B）；TxPDO = statusword + position
  + velocity（10B）+ 可選 0x60FD digital inputs（+4B，極限/原點開關）。
  進 OP 後逐軸驗 PDI 區塊長度，不符立即失敗（拒絕整場靜默錯位解碼）。
- cyclic 順序：寫輸出（用**上一週期**的 statusword 算 controlword）→ tx/rx →
  讀輸入。controlword 慢一拍是 402 握手天生容忍的。
- drive 未進 Operation Enabled 期間，target 持續鏡射 actual position ——
  CSP 使能瞬間永不跳步（motion-core 端 set_pos 同步是第二道防線）。
- `fault_code` 暫發 0（0x603F 要走 mailbox，見 ADR-12）；DriveStatus::Fault
  本身已足夠驅動 motion-core 的錯誤路徑。

**理由**：固定佈局 = 解碼零查表零分支；每軸 6B/14B 的 PDI 拷貝讓 PDI
spinlock guard（ethercrab 0.6）持有時間最短化。

## ADR-12: EtherCAT acyclic 通道暫回 Unsupported（與 homing 一起做）

**決策**：`EthercatAcyclic` 目前回 `Unsupported`。SDO 設定需求由 start() 的
PREOP 階段涵蓋（CSP 模式、PDO mapping）。

**理由**：ethercrab 的 `SubDeviceRef::new` 是 crate-private，SDO 只能透過
group 借用取得，而 group 被 cycle 路徑獨佔；mailbox 往返又絕不能佔用 1ms
cycle。正解是 cycle thread 服務的請求佇列（每週期處理一小步），這與 homing
參數（`MC_WriteHomingParameters` 領域知識）是同一件工作，屆時一起落地，
不為了「介面好看」先塞一個會破壞 RT 的實作。

## Phase 3 狀態與 bring-up

`fieldbus-ethercat` 已實作：拓撲掃描 → PREOP（CSP 模式、PDO mapping、可選
DC SYNC0）→ `request_into_op` + 邊跑 cyclic 邊等全體 OP（餵 watchdog）→
1ms cyclic PDO 迴圈接進 trait；自有 `cia402` 模組（ADR-4）負責
statusword/controlword。雙主站 = 兩個 `EthercatBackend` 實例（每實例自帶
leak 一份 `PduStorage`、綁自己的 NIC），無全域狀態。

bring-up example（實機驗證入口，kickoff §6 的 jitter/DC 數據由此輸出）：

```
cargo build --release --examples -p fieldbus-ethercat
sudo ./target/release/examples/bringup --ifname eth0 \
    [--cycle-us 1000] [--duration-s 10] [--axes 1] [--first-subdevice 0] \
    [--scale 10000] [--no-dc] [--no-din] [--no-pdo-config] [--enable]
```

輸出：拓撲清單（address/identity/name）、wake latency / period jitter /
exchange time 的 min/mean/p99/max + 直方圖、offline/WKC 錯誤計數、各軸終態。
`--enable` 會走 402 使能鏈並以 CSP 鎖住當前位置（無運動）。無硬體時的行為：
掃描 timeout、乾淨退出（已在 WSL 驗證）。daemon 端：
`motion-daemon --backend ethercat --ifname eth0`（`scale`/`ecat_dc`/
`first_axis_subdevice` 走設定檔），進 cycle loop 前套 mlockall + SCHED_FIFO。

## 已知待辦（Phase 3 之後）

- **實機驗證**（kickoff §6）：DC 同步品質/漂移統計、drive 相容性（PDO mapping
  接受度、0x60FD 有無）、`--enable` 使能鏈實測 —— bring-up example 就是為此。
- acyclic 請求佇列 + homing（ADR-12；`MC_WriteHomingParameters` /
  `MC_ActualDriver_SetPosition` 的 LC10E 領域知識）。
- 雙主站組裝：daemon 目前單 bus 實例；trait/adapter 已 per-bus，組裝層加
  `Vec<(bus, axis 映射)>` 即可。
- rsruckig 替換 `profile.rs`（Phase 2）；與 C++ Ruckig 對拍。
- TargetForwarding 策略分支 + fieldbus-modbus。
- 0x603F/0x1003 錯誤碼經 acyclic 讀回填 `AxisIn::fault_code`。
