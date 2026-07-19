# codesys_dev — PLC 橋接器範本

工業電腦上 **CODESYS PLC + Go 橋接器 + Svelte HMI** 的整合堆疊。`main` 是
**機種無關的乾淨基底**：基礎建設（shm IPC、Modbus、WebSocket、角色登入、TLS、
i18n、CI、部署）齊備，但**不含任何機種業務邏輯**。要做一台新機器，從 `main`
分支，照 [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) §9 加東西。

開發踩雷與模式、以及「從這個 base 長出一台新機器」的步驟，集中在
**[docs/DEVELOPMENT.md](docs/DEVELOPMENT.md)**。

## 架構

三個獨立構建、獨立部署的組件：

```
+----------------------+         +-------------------+         +----------------------+
|  codesys_export/     |  shm    |  backend/         |  HTTPS  |  frontend/           |
|  IEC ST 程式碼       | <-----> |  Go 服務          | <-----> |  Svelte 5 HMI        |
|  (CODESYS Edge)      | /dev/   |  (HTTPS + Modbus) | :8443   |  (Chromium kiosk)    |
|                      |  shm    |                   |         |                      |
+----------------------+         +-------------------+         +----------------------+
       runs on PLC                  runs on Linux                 embedded into Go
       runtime (RT cores)           userspace (CPU 0-1)           binary via go:embed
```

## 資料夾

| 路徑 | 角色 | 部署方式 |
|---|---|---|
| [backend/](backend/) | Go 服務：讀寫 `/dev/shm/plc_data` 與 `/dev/shm/plc_cmd`、提供 HTTPS WebSocket 給 HMI、Modbus TCP server 給 SCADA、角色密碼 auth。 | Makefile `make deploy`（scp + ssh）到 `/opt/plc_bridge/`，systemd unit [scripts/plc_bridge.service.unit](scripts/plc_bridge.service.unit)。本機 WSL 走 `make wsl-deploy`。 |
| [frontend/](frontend/) | Svelte 5 HMI。`npm run build` 產生 `frontend/dist/`，由 [frontend/embed.go](frontend/embed.go) 用 `//go:embed` 吞進 Go binary。 | 不獨立部署，跟著 backend binary 走。 |
| [codesys_export/](codesys_export/) | CODESYS 專案的文字投影。每個 IEC object（DUT / GVL / POU）一個 `.st` 檔，可進 Git、用 VS Code 編輯。 | 用 [cds-text-sync](https://github.com/kevin00156/cds-text-sync) 在 CODESYS `.project` 與這些 `.st` 之間雙向同步（見下）。 |
| [csharp/](csharp/) | Go 服務的 **C#/.NET 8 對等實作**：同一份 shm 合約、同一組 wire format，e2e harness 與前端不用改就能對接。見下方「一份合約、多個實作」。 | `dotnet build csharp/PlcBridge`；部署方式與 Go 版相同（單一 service binary）。 |

## CODESYS 文字同步（cds-text-sync）

CODESYS 的 `.project` 是專有二進位格式，難進 Git、難用外部編輯器改。
[**cds-text-sync**](https://github.com/kevin00156/cds-text-sync) 把 POU / GVL / DUT 投影成
`.st` 文字檔並**雙向同步**，IEC 程式碼就能用 VS Code / Cursor 編、用 Git 版控。
**建議搭配本範本使用** —— `codesys_export/` 就是它的輸出。

安裝（PowerShell，裝進 CODESYS ScriptDir）：

```powershell
irm https://raw.githubusercontent.com/kevin00156/cds-text-sync/main/irm/setup.ps1 | iex
```

工作流：

1. `Project_directory.py` — 把 CODESYS 專案綁到磁碟資料夾（本 repo 的 `codesys_export/`）
2. `Project_parameters.py` — 設定匯出 / 備份選項
3. `Project_export.py` — CODESYS → `.st`（commit 前跑）
4. 用 VS Code / Cursor 改 `.st`
5. `Project_import.py` — `.st` → CODESYS（把外部改動同步回 IDE）

> `sync_metadata.json` / `sync_cache.json` 是它的同步狀態（Merkle 雜湊），屬本地產物，已 gitignore。

## 跨層契約：shm layout

PLC 與 Go 透過兩塊共享記憶體通訊，byte layout 必須兩邊一致：

| Go 端 | IEC 端 |
|---|---|
| [backend/internal/shm/layout.go](backend/internal/shm/layout.go) | [codesys_export/Device/Application/DUT/ShmBridge/](codesys_export/Device/Application/DUT/ShmBridge/) |

兩邊各自定義 `Magic` 與 `Version`。**改動 layout 必須兩邊同步 bump version**，否則 Go reader 會直接拒絕掛載 segment（不會悄悄讀垃圾資料）。詳細流程見 [backend/README.md](backend/README.md)。

## 一份合約、多個實作

shm 佈局 + wire format（WS JSON、Modbus 位址表、auth endpoints）是**合約**；
橋接器只是合約的實作，語言可以替換：

| 實作 | 路徑 | 狀態 |
|---|---|---|
| Go | [backend/](backend/) | 參考實作，生產部署 |
| C# / .NET 8 | [csharp/](csharp/) | 完整對等實作：同一個 e2e harness（[backend/e2e_smoke](backend/e2e_smoke/main.go)）一行不改 ALL PASS，golden 檔逐 byte 對齊 |

跨語言對齊靠兩道閘：**golden 檔測試**（`backend/internal/shm/testdata/*.bin`，
各實作把同一組結構體編碼成 bytes 逐一比對）與 **e2e smoke harness**（模擬 PLC，
驗證登入 / WS 活資料 / jog watchdog / stale 偵測全鏈路）。前端與 PLC 完全不知道
（也不需要知道）後端是哪個語言。

## 構建與部署

```sh
make build           # 先編 frontend dist，再編 Go binary（embed 順序，見 docs §3）
make deploy          # build 後 scp+ssh 到工業電腦並重啟 systemd
make rollback        # 切回 .previous binary
make logs            # 串流遠端 journalctl
make clean           # 清除 backend/dist 與 frontend/dist

make wsl-bootstrap   # 一次性：在 WSL 建 plc_bridge user / unit / sudoers
make wsl-deploy      # 本機 WSL fallback 部署（免 scp）
make shm-reset       # layout 改版後清掉 /dev/shm/plc_* 再重啟（wsl-shm-reset 同理）
make cert            # 產自簽 TLS 憑證（SAN 取自 PLC_HOST）
make deploy-certs    # 安裝憑證到 /opt/plc_bridge/，TLS 自動啟用
```

部署目標用 `.env`（從 [.env.example](.env.example) 複製）設定 `PLC_HOST` / `PLC_USER` /
`WSL_DISTRO`，或單次 `make deploy REMOTE=user@host` 覆寫。

> **登入密碼**：範本內建預設密碼 **`111111`**（唯讀儀表板免登入，操作機台才需登入；
> 登入頁會顯示這組預設密碼與變更步驟）。**正式部署請務必變更**：用
> `plc_bridge -gen-hash` 產生 bcrypt hash，寫進 service env file 的
> `PLC_BRIDGE_PASSWORD_HASH`（細節見 [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) §4）。

## 開發環境

- 開發機（Windows + WSL）：開發、跨平台編譯（Go 在 host、frontend 在 WSL/host）
- 工業電腦（例：`192.168.1.10`）：CODESYS Edge + PREEMPT_RT kernel + CPU isolation（`isolcpus=2-3` 給 PLC 用，CPU 0-1 給 backend）
- 部署與環境細節見 [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) §7

## 用這個範本開新機器

這是一個 **GitHub Template Repository**。按 **Use this template** 生成你自己的 repo
（建議設私有），就有一份乾淨的 PLC 橋接器骨架。接著照
[docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) §9 加上你機種的 shm 欄位、後端路由、
前端畫面與 IEC 程式碼。

## 授權

[MIT](LICENSE) — 可自由使用、修改、散布（含商業用途），保留著作權聲明即可。
本範本不含任何機種專屬業務邏輯或機密；自簽憑證、`.env`、本地狀態皆已 gitignore。
