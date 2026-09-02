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
cargo clippy --workspace --all-targets   # 零警告是驗收條件
```

Windows dev box 沒有 cargo 時，在 WSL（Ubuntu-22.04）裡跑同樣的指令；
`fieldbus-ethercat` 的真 backend、bring-up examples 與 `shm-bridge` 的
`/dev/shm` 測試只在 Linux 編譯與執行。

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

Writer 端的 crash recovery 與 Go `writer.go` 相同：起手的 in-progress 值是
`s + 1 + (s & 1)`，不是 `s + 1`。殘留奇數 seq（上一個 writer 在兩次 seq store
之間被殺）用 `s + 1` 會產生**偶數**的「寫入中」標記，收尾再把段落停在奇數，
之後每次 snapshot 都 `Busy`、沒有任何錯誤訊息。回歸測試釘住。

其他 shm 決策（照 kickoff §4，2026-09 審查後修訂）：
- `plc_data` / `plc_cmd` 由 daemon **重用既有 inode**（`Mapping::open_or_create`
  + `seqlock::reset`），不再 unlink 重建。原因：Go 橋接器啟動時 mmap 一次、
  之後不重開；unlink 重建會讓橋接器永遠讀寫一個孤兒 inode（資料凍結、命令
  送不到），直到有人重啟橋接器。`reset` 在 seqlock 協議下把 payload 清零
  （magic 0 = 「尚無 writer」），競爭中的 reader 只會看到「上一份好快照」或
  乾淨的 `MagicMismatch`，不會拿到新舊混雜的撕裂快照；seq 從舊值接續。
  `plc_trace` 仍走 `Mapping::create`（unlink 重建），因為它的 Go reader
  監看 inode 變更會自動重 mmap，而且新環要的是乾淨零頁。
  開檔權限直接給足（plc_data 0644 / plc_cmd 0666），退出時**不 unlink**。
- `plc_cmd` header 不由 daemon 初始化（PRG_ShmPublisher 也沒有）：magic 無效
  就當「橋接器還沒上線」，杜絕撿到舊 segment 的殘留命令。
- 命令消費照 `PRG_ShmPublisher.st`：seqlock 快照 + **`header.cycle` 變化才
  整包 latch**；Go 端 `cmdSink` 每次接受命令 bump cycle。旗標因此是
  level-held——HMI 按住 jog = 舊 word 持續有效，放開送 word=0。
- **每軸 fresh**：`PlcCommand.header.flags` 由 Go `cmdsink` 填入「本次訊息
  觸碰了哪幾軸」的遮罩（bit15 = 遮罩有效、bit0..3 = 軸 i）。daemon 只把
  `fresh` 給被指名的軸；沒有 bit15 的舊 writer 退回「每軸都 fresh」。沒有
  這個遮罩時，`header.cycle` 是整包訊息的，任何無關訊息（另一軸的 jog 每
  250 ms 重送、機台 Reset、watchdog 重發）都會 re-arm 別軸仍鎖存的 MOVE_ABS
  / HOME 位元——MOVE_ABS 只是 Step 閃爍，HOME 在真驅動器上是重新跑一次歸原點。
  layout 不變（`Flags` 本來就存在），CODESYS 端忽略它。
- **命令 dead-man**（daemon 端）：Go 的 jog watchdog 只保護「前端消失」，
  plc_bridge 自己被殺時 daemon 會一直持有最後鎖存的 jog 字。前端每 250 ms
  重送 jog 會 bump cycle，所以 daemon 用「有 jog 位元但超過 `cmd_timeout_ms`
  （預設 1 s）沒有新訊息」剝掉 jog 位元、亮 `system.alarm_flags` bit1，
  下一筆新訊息即解除。EMS 等 level-held 位元不受影響（故障方向要保守）。
  Go 端另外把 `cycle` 從既有段落接續（bridge 重啟不會重發一個 daemon
  已鎖存過的 cycle 值）。

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
3. **驅動器在 ENABLING 之後任何步驟掉出 `Enabled`（Disabled / Enabling /
   QuickStop / Offline，不只 Fault）都 raise `DRIVER_ERROR`**。Main.st 靠
   SoftMotion FB 的 Error 輸出抓這種情況；我們沒有 FB，只看 Fault 會讓 jog
   中 STO 觸發後 set_pos 繼續積分、重新使能瞬間 CSP 跳步。TRY_RESET 回
   READY 也要求 `drive == Enabled`。
4. **HOMING_* 與 TRY_RESET（drive 未使能時）每週期 set_pos = act_pos**：
   歸原點期間軌跡由驅動器持有，取消或 EMS 轉 STOPPING 時 CSP 目標必須從
   實際位置起算，否則會命令跳回歸原點前的位置。
5. **MOVE_ABS 執行中收到新（fresh）MOVE_ABS 就重定目標**，梯形從目前
   set_pos / set_vel 續走（反向煞車用 dec）。否則第二次 Go 會被 re-arm latch
   吃掉。

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

## ADR-13: 週期健康度 — WKC 雙重檢查、DC 相位鎖定、overrun 重同步

**決策**（`backend.rs` + `motion-daemon/engine.rs`，2026-09 審查後）：
- **每週期兩道檢查**：全體 subdevice AL 狀態 = OP **且** LRW working counter
  等於進 OP 那一週期學到的值（`WkcMonitor`）。ethercrab 只回傳 WKC 不驗證；
  鏈尾節點物理消失時 AL 位圖仍 OR 成 OP，只有 WKC 會掉。任一道失敗 →
  該週期各軸 `DriveStatus::Offline`、`ExchangeStatus::working_counter_ok =
  false`、trace `status_bits` b4。
- **DC 模式主站週期由 SYNC0 定速**：`tx_rx_dc` 回傳的 `CycleInfo::
  next_cycle_wait` 經 `ExchangeStatus::next_cycle_wait` 交給 daemon，daemon
  以「exchange 前的時間戳 + wait」排下一次喚醒（ethercrab 文件的用法）。
  之前用主機時鐘自由跑，送幀時間相對 SYNC0 漂移，跨過邊界時驅動器在一個
  SYNC0 內收到 0 或 2 個設定點。
- **overrun 重同步**：喚醒晚於 deadline 超過一整個週期就計數並把 `next`
  重設為 `now + cycle`，不讓 `Timer::at` 連續立即觸發把積壓的設定點壓縮
  送出（trace `status_bits` b3）。
- **PDU 逾時 = max(4 × cycle, 2 ms)**：原本 100 ms 與 subdevice SM watchdog
  同量級，丟一個幀就卡住約 100 個週期。
- **匯流排故障時持續發布**：exchange 失敗不 tick 狀態機（輸入是舊的），但
  仍 latch 命令（EMS 進得來）並每週期發布 `plc_data`，`system.alarm_flags`
  bit0 亮起、cycle 持續前進。HMI 因此分得出「daemon 死了」與「匯流排斷了」。

## ADR-14: 優雅關機 — 先煞車再斷使能再降階

SIGTERM/SIGINT 不再立即跳出 cycle loop。`engine.shutdown_step` 分兩段：
(1) 強制 EMS 持續 exchange，直到所有軸回報 STANDSTILL 或 `shutdown_timeout_ms`
（預設 2 s）；(2) 十個週期 `enable = false`，讓 402 sequencer 走 Shutdown
轉移而不是 disable voltage 滑行；然後 `bus.stop()`：五個週期 controlword 0
→ `into_safe_op` → `into_pre_op`，subdevice 被停在 PREOP，而不是靠 SM
watchdog 掉出 OP 記一筆 0x001B 通訊錯誤。第二次訊號直接跳到 `bus.stop()`
（systemd `TimeoutStopSec` 之後反正會 SIGKILL）。部署用
`scripts/motion-daemon.service.unit`（CPUAffinity=3、SCHED_FIFO 80、
LimitMEMLOCK=infinity、`Conflicts=codesyscontrol.service`）與
`scripts/motion-daemon.conf.example`。

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
exchange time 的 min/mean/p99/max + 直方圖、offline / WKC / exchange 錯誤計數、
各軸終態。`--enable` 會走 402 使能鏈並以 CSP 鎖住當前位置（無運動）；
Ctrl-C 會走正常的 `stop()`，報表在 `stop()` 之後才印（驅動器使能中不留
無幀空窗）。無硬體時的行為：掃描 timeout、乾淨退出（已在 WSL 驗證）。
daemon 端：`motion-daemon --backend ethercat --ifname eth0`（`scale`/
`ecat_dc`/`first_axis_subdevice` 走設定檔），進 cycle loop 前套 mlockall +
SCHED_FIFO；設定值有範圍驗證（非有限數、零減速度、零 scale 一律拒絕）。

實機診斷工具（`examples/pdo.rs`、`examples/sdo.rs`）：pdo console 的輸出
寫入是**白名單**（`--writable <node>[,..]`，vendor 0xF9 的 CT drive 即使列出
也硬鎖），Ctrl-C 與 `q` 都會歸零輸出再降階到 PREOP，`stats` 含 WKC 期望/
實際/錯誤與 overrun 計數；sdo console 對 vendor 物件（0x2000..0x5FFF）的寫入
預設拒絕，需 `--allow-vendor-write`（CT drive 的參數在 PREOP 下寫入即生效）。

## 已知待辦（Phase 3 之後）

- **非 402 驅動器的 drive profile 抽象 + IO-only 子裝置**：實機的 CT drive
  不是 CiA 402（寫 0x6060 會被拒），MKX 1313 是純 IO（極限/原點開關在它身上，
  不在驅動器的 0x60FD）。需要 per-axis 的 profile（402 CSP vs vendor PDO
  佈局，由設定描述 offset）與「數位輸入來自另一個 subdevice」的對映，daemon
  才能在那台機器上跑。這是範本與實機之間最大的缺口。
- **實機驗證**（kickoff §6）：DC 相位鎖定實測（ADR-13 的 `next_cycle_wait`
  配速）、WKC 學習值是否等於 3 × 有 in/out 的 subdevice 數、`into_safe_op`
  / `into_pre_op` 降階對 CT drive 與 MKX 的反應、drive 相容性（PDO mapping
  接受度、0x60FD 有無）、`--enable` 使能鏈實測 —— bring-up example 就是為此。
- acyclic 請求佇列 + homing（ADR-12；`MC_WriteHomingParameters` /
  `MC_ActualDriver_SetPosition` 的 LC10E 領域知識）。
- 位置以 i32 增量傳遞，長行程或高解析編碼器的 0x6064 溢位（multi-turn
  unwrap）尚未處理。
- 雙主站組裝：daemon 目前單 bus 實例；trait/adapter 已 per-bus，組裝層加
  `Vec<(bus, axis 映射)>` 即可。
- rsruckig 替換 `profile.rs`（Phase 2）；與 C++ Ruckig 對拍。
- TargetForwarding 策略分支 + fieldbus-modbus。
- 0x603F/0x1003 錯誤碼經 acyclic 讀回填 `AxisIn::fault_code`。
