# C# 後端移植（feat/csharp-bridge）

目標：`csharp/` 新增 plc_bridge 的 C#/.NET 8 實作 —— 同一份 shm 佈局合約、同一組
HTTP/WS/Modbus 對外行為，Go 實作（`backend/`）原封不動。驗收 = 既有 e2e smoke
harness 一行不改跑通 + golden 檔逐 byte 對齊。

移植參考：feat/rust-motion 的 backend（含 PR#3 缺陷修復、cycle 計數 staleness、
cmdsink jog watchdog）。main 的 backend 缺這些修復，不作為參考。

## 前置

- [x] 分支 feat/csharp-bridge 基於 main
- [x] .NET 8 SDK（~/.dotnet，8.0.423）
- [x] cherry-pick e2e harness（fbcafcc → 36fbbc0）
- [x] layout.go 補 AxisCtrl 常數（與 feat/rust-motion 逐字相同，harness 編譯需要）
- [x] 通讀參考版 backend 全部原始碼與測試

## 移植（csharp/PlcBridge）

- [x] 專案骨架：PlcBridge.csproj（net8.0, unsafe）+ PlcBridge.Tests.csproj（xunit）
- [x] Shm/Layout.cs — struct 佈局（LayoutKind.Sequential + InlineArray），248/168 B
      大小驗證（vet.go 對應，啟動時 VerifySizes）
- [x] Shm/Mapping.cs — /dev/shm mmap（MemoryMappedFile + AcquirePointer）＋
      測試用 in-memory mapping（NativeMemory.AlignedAlloc）
- [x] Shm/Seqlock.cs — ReadPlcData / WritePlcCommand，Volatile.Read/Write，
      奇偶協議與 crash-recovery（odd-skip）語義逐行對應，x86 TSO 註記照搬
- [x] State/Snapshot.cs — cycle 計數器判 staleness（不是讀取成功與否）
- [x] CmdSink/Sink.cs — pending 命令 + 錯誤 rollback + jog watchdog（timeout/4 tick）
- [x] Modbus/ — MBAP 框架、fn 03/06/10、位址表、寫入驗證（partial/unmapped 拒絕）、
      stale → exception 04
- [x] Ws/ — 推送（semaphore 單一 writer）、ack（DropWrite channel）、AuthorizeWrite
      門禁、同源檢查、JSON 欄位名逐字對齊
- [x] Auth/ — bcrypt（BCrypt.Net-Next cost 10，相容既有 PLC_BRIDGE_*_HASH）、三角色、
      session cookie 語義（HttpOnly/Strict/12h）、預設密碼 111111 + nag
- [x] Program.cs — Go 風格 flags（含 Go duration 語法）、TLS 自動偵測、組裝

## 驗證

- [x] 單元測試移植：seqlock（含 concurrent torn-read 2M 寫入）、golden（同一份
      testdata *.bin）、addresses、cmdsink、snapshot、applyCmd、auth
- [x] dotnet test 全綠（33/33）
- [x] e2e harness 對 C# bridge 跑通 —— **ALL PASS，harness 一行未改**
      （登入/WS 活資料/jog watchdog/Modbus exception 04/WS stale/復活）
- [x] ultracode 審查 workflow：4 維度（記憶體序/協議對齊/安全/runtime）×
      每 finding 3 反駁者投票 —— 20 findings，17 confirmed 全數修復，3 refuted
- [x] 修復後重驗：dotnet test 33/33、e2e ALL PASS、trailing slash/HEAD/
      null body/-h/-stale-after 0/localhost 抽查全過

## 收尾

- [x] 根 README 補「一份合約、多個實作」段落 + csharp/ 資料夾列
- [x] csharp/README.md（對應表、刻意差異、記憶體序說明）
- [x] tasks/todo.md 補 review 段
- [x] commit

## Review（審查 workflow 結果與修復）

64 個 subagent（4 個維度審查者 + 每 finding 3 個獨立反駁者，多數有實際跑
雙橋 A/B 對照驗證）。20 findings → 17 confirmed（全修）、3 refuted。

修復清單（全部落地並重驗）：

| # | 嚴重度 | 問題 | 修法 |
|---|---|---|---|
| 1 | HIGH | TLS 只送 leaf 憑證（CreateFromPemFile 只讀第一個 block），fullchain 部署下嚴格 client 握手失敗 | `ImportFromPemFile` 全鏈 + `ServerCertificateChain` |
| 2 | HIGH | SendJson 序列化在 try 外：PLC 發 NaN → pushTask 無聲 fault，殭屍連線 | 序列化搬進 try，失敗斷線（Go WriteJSON 語義） |
| 3 | MED | 看門狗 fire-and-forget 吞非 OCE 例外：dead-man 無聲失效 | catch-all → log + exit(1)（fail loud，同 Go panic） |
| 4 | MED | pollTask 只 catch OCE：輪詢死了橋還活著、關機時炸 Main | catch-all → log + cts.Cancel |
| 5 | MED | SIGTERM 後 WS 連線讓關機卡 30 秒（Kestrel 優雅逾時） | 每連線 cts 連結 Shutdown token |
| 6 | MED | trailing slash 路由 C# 放行、Go 404 | /api/*/、/ws/ 短路 404（Go http.NotFound 格式） |
| 7 | MED | HEAD /api/auth/status C# 405、Go 200 | MapMethods GET+HEAD，HEAD 不寫 body |
| 8 | MED | login body 解析過嚴（trailing garbage、`null`） | Utf8JsonReader 單值解析（Go Decoder 語義） |
| 9-17 | LOW | WS `null` ack 字串、1MiB cap、Origin 預設埠、hostname 位址崩潰、CLI -h/positional/bool、gen-hash 72B、cert.pem 目錄誤判、duration "0" | 逐一對齊 Go 行為 |

Refuted（不修）：seqlock Volatile 記憶體序質疑（協議正確）、Mapping 二次
Dispose（using 模式下不可達）、JSON byte 級指紋（對 client 無行為差異）。

## 已知且刻意的差異（誠實記錄）

- trace/watch 調試設施不移植（feat/rust-motion 專屬，非核心鏈路）
- 前端改為磁碟路徑服務（Go 是 embed；C# 用 -webroot / 自動偵測 frontend/dist）
- WS keepalive：.NET 8 WebSocket 無 server 端 ping-with-deadline API，改用
  KeepAliveInterval + 寫入逾時斷線（Go 是 ping/pong deadline）
