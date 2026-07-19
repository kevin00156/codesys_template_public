# plc_bridge — C# 實作

[backend/](../backend/)（Go 版 plc_bridge）的 **C#/.NET 8 對等實作**。同一份 shm
佈局合約、同一組對外行為 —— Svelte 前端、Modbus SCADA master、以及
[backend/e2e_smoke](../backend/e2e_smoke/main.go) 驗收 harness **分不出**自己在跟
哪個實作講話。

```
一份合約（shm layout + wire format），多個實作：
  backend/   Go      —— 生產部署的參考實作
  rust/      Rust    —— motion/trace 子系統移植（feat/rust-motion 分支）
  csharp/    C#      —— 本目錄
```

## 對應表

| C# | Go | 內容 |
|---|---|---|
| [PlcBridge/Shm/Layout.cs](PlcBridge/Shm/Layout.cs) | `internal/shm/layout.go` + `vet.go` | IEC DUT 鏡像結構（`LayoutKind.Sequential` + `[InlineArray]`），248/168 B，啟動時 `VerifySizes()` |
| [PlcBridge/Shm/Mapping.cs](PlcBridge/Shm/Mapping.cs) | `internal/shm/mapping*.go` | `/dev/shm` mmap（`MemoryMappedFile` + `AcquirePointer`） |
| [PlcBridge/Shm/Seqlock.cs](PlcBridge/Shm/Seqlock.cs) | `internal/shm/reader.go` + `writer.go` | seqlock 讀寫：`Volatile.Read/Write`、奇偶協議、crash-recovery odd-skip。記憶體序注記（x86 TSO 前提）與 Go 版逐字對應 |
| [PlcBridge/State/Snapshot.cs](PlcBridge/State/Snapshot.cs) | `internal/state/snapshot.go` | 快照 + **以 PLC cycle 計數器判 staleness**（不是 shm 讀取成功與否） |
| [PlcBridge/CmdSink/Sink.cs](PlcBridge/CmdSink/Sink.cs) | `internal/cmdsink/cmdsink.go` | 指令序列化、錯誤 rollback、jog watchdog（dead-man） |
| [PlcBridge/Ws/](PlcBridge/Ws/) | `internal/wsserver/` | WebSocket 推送 + 指令，JSON 欄位逐字相同，同源檢查（gorilla CheckOrigin 對應） |
| [PlcBridge/Modbus/](PlcBridge/Modbus/) | `internal/modbus/` | 零依賴 Modbus TCP slave（fn 03/06/10），同一張位址表，stale → exception 04 |
| [PlcBridge/Auth/Authenticator.cs](PlcBridge/Auth/Authenticator.cs) | `internal/auth/auth.go` | 三角色 bcrypt 登入。**同一組 `PLC_BRIDGE_*_HASH` 環境變數、同一格式 bcrypt hash** —— 換實作不用換密碼 |
| [PlcBridge/Program.cs](PlcBridge/Program.cs) | `cmd/plc_bridge/main.go` | 同一組 CLI flags（Go 語法 duration）、TLS 自動偵測、預設密碼 111111 |

單元測試（[PlcBridge.Tests/](PlcBridge.Tests/)）是 Go 測試的逐一移植，包括：

- **golden 檔測試**：讀 `testdata/*.bin`（與 `backend/internal/shm/testdata/`
  同一份檔案），證明 C# 結構體編碼出的 bytes 與 Go / Rust / IEC 完全一致
- **seqlock 撕裂讀並發測試**：writer 對 reader 真實並行 200 萬次寫入

## 建置與執行

```sh
dotnet build csharp/PlcBridge                # 建置
dotnet test  csharp/PlcBridge.Tests         # 單元測試
dotnet run --project csharp/PlcBridge -- -jog-timeout 500ms -stale-after 500ms
```

e2e 驗收（與 Go 版同一個 harness，一行不改）：

```sh
python3 -c "open('/dev/shm/plc_data','wb').write(b'\0'*248); open('/dev/shm/plc_cmd','wb').write(b'\0'*168)"
dotnet run --project csharp/PlcBridge -- -jog-timeout 500ms -stale-after 500ms &
go run ./backend/e2e_smoke     # 印 PASS 行，失敗非零退出
```

## 刻意的差異（誠實記錄）

| 差異 | 原因 |
|---|---|
| trace/watch 調試設施未移植 | feat/rust-motion 分支的子系統，非核心鏈路 |
| 前端從磁碟服務（`-webroot`，自動偵測 `frontend/dist`） | Go 用 compile-time `go:embed`；.NET 對等物（資源嵌入）成本高於價值 |
| WS keepalive 用 `KeepAliveInterval` + 寫入逾時斷線 | .NET 8 的 server WebSocket 沒有 ping-with-deadline API（.NET 9 才有 `KeepAliveTimeout`）；Go 版是 ping/pong read deadline |
| 唯一 NuGet 依賴：`BCrypt.Net-Next` | 必須與 Go `golang.org/x/crypto/bcrypt` 產的 hash 互通（never break userspace）；密碼學不自己寫 |

## 記憶體序

seqlock 的正確性前提與 Go 版完全相同：**x86-64 TSO**（load-load、store-store 不重排）。
`Volatile.Read/Write` 提供 acquire/release 與編譯器屏障，payload 的普通複製依賴
硬體 TSO —— 與 Go 版 `sync/atomic` + 普通 copy 的組合一字不差。要移植到 ARM，
兩個實作都需要補 fence（見 [Seqlock.cs](PlcBridge/Shm/Seqlock.cs) 註解）。
