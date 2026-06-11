# 開發經驗 / Development Guide

這份文件把這套 **CODESYS PLC + Go 橋接器 + Svelte HMI** 堆疊在多台機器上累積的
踩雷與模式集中起來。`main` 是**機種無關的乾淨範本**：只有基礎建設與這些經驗，
沒有任何機種業務邏輯（飛剪、橫切機等各自活在自己的分支）。要長出一台新機器，
從這裡分支，照 §9 加東西。

更長的部署 / WSL 流程筆記見 `.claude/skills/deploy-strategy`、`wsl-deploy-strategy`。
作者私有的機器規格與內部部署狀態（特定 IP / 硬體）未隨公開範本附帶。

---

## 1. 架構與資料流

```
  CODESYS RT task            plc_bridge (Go)                 Svelte HMI (Chromium kiosk)
        │                          │                                 │
        │ writes PlcData (seqlock) │  /ws  WebSocket push (50–100ms)  │
        ▼                          │ ◄───────────────────────────────┤ commands (CmdMsg)
  /dev/shm/plc_data ──────────────►│  Modbus TCP :5020 → SCADA        │
  /dev/shm/plc_cmd  ◄──────────────┤  HTTP/HTTPS :8443 → embedded UI  │
                                   │  role-password auth + TLS        │
```

- **PLC ↔ Go 只靠兩塊 shm**（`plc_data` 唯讀、`plc_cmd` 命令）。其餘（Modbus、JSON、
  WebSocket、auth）全在 Go，PLC 完全不知情。加新外部介面**永不動 PLC**。
- 前端 `npm run build` 產出 `frontend/dist/`，被 `frontend/embed.go` 用 `//go:embed`
  吞進 Go binary。HMI 不獨立部署，跟著 binary 走。

---

## 2. shm 跨層契約（最容易出事的地方）

`PlcData` / `PlcCommand` 的 byte layout 必須 Go 與 IEC 兩邊**逐位元組一致**：

| Go 端 | IEC 端 |
|---|---|
| [backend/internal/shm/layout.go](../backend/internal/shm/layout.go) | `codesys_export/Device/Application/DUT/ShmBridge/*.st` |

鐵律：

1. **改 layout 一定兩邊同步 bump `Version`**。版本對不上，Go reader 直接拒絕掛載
   segment（不會悄悄讀垃圾）。新欄位**附加在尾端、舊 offset 不動**。
2. **seqlock 防撕裂讀**：writer 寫入期間把 `Header.Seq` 設成奇數，reader 看到奇數
   或前後 Seq 不一致就重讀。見 [writer.go](../backend/internal/shm/writer.go) /
   [reader.go](../backend/internal/shm/reader.go) 與 `seqlock_test.go`。
3. **`vet.go` 用編譯期 assert 釘住 struct 大小**——欄位順序/型別一漂移就編不過。
4. **mmap 是 Linux-only，但整包要能在 Windows dev box 編譯**：OS 相關碼分到
   `mapping_linux.go`（`//go:build linux`，真 mmap）與 `mapping_other.go`
   （`//go:build !linux`，stub 回 error）。這個 stub 就是讓 `go build ./...` 在
   Windows / macOS 上過的關鍵——少了它，每個平台都編不動。

---

## 3. Build 順序陷阱

`backend/cmd/plc_bridge` `import` 了 `frontend`（embed 包），而 `//go:embed dist`
要求 build 當下 `frontend/dist/` 必須存在。所以：

> **永遠先 `npm run build`（產生 dist），再 `go build`。**

`make build` 已照此順序。**CI 也踩過這個雷**：原本把 backend / frontend 拆成兩個
平行 job，backend job 沒先 build 前端，`go vet ./backend/...` 一碰 plc_bridge 就因
`pattern dist: no matching files found` 而紅。本範本的 CI（[.github/workflows/ci.yml](../.github/workflows/ci.yml)）
改成**單一 job**：先 node build 前端 → 再 `go vet`/`go test`/linux 交叉編譯。

---

## 4. 角色密碼 Auth

[backend/internal/auth](../backend/internal/auth) — 三級權限，刻意極簡（無使用者帳號，
每級一個 bcrypt hash + 記憶體內 session token）：

- **operator 操作員** → **tuner 調機** → **vendor 廠商**（後者涵蓋前者）。
- hash 從環境變數來：`PLC_BRIDGE_PASSWORD_HASH`（vendor）、`_TUNER_HASH`、
  `_OPERATOR_HASH`。用 `plc_bridge -gen-hash`（從 stdin 讀密碼、印 bcrypt hash）產生，
  寫進 service env file——明文永不進 args / shell history / git。
- **全空 = auth 關閉**，每條路由全開（dev 姿態）；`/api/auth/status` 回 `enabled:false`。
- **operator 級是 opt-in**：沒設 operator hash 時，operator 級路由維持開放，舊的
  兩級部署行為不變。
- session cookie `HttpOnly` + `SameSite=Strict`；**只在 TLS 下標 `Secure`**。
- **TLS 自動啟用**：cwd 有 `cert.pem`/`key.pem` 就走 HTTPS（systemd unit 與
  `plc_bridge` 二進位都這樣偵測）。`make cert` 產自簽憑證，`make deploy-certs` 安裝。
- 路由門禁是**單一 chokepoint**：`auth.Wrap(mux, requiredRole)`。範本的 `requiredRole`
  回 `RoleNone`（無受保護路由），加機種 API 時在那裡 gate 寫入（範例見 main.go 註解）。
  前端藏分頁只是化妝，**後端 Wrap 才是牆**。

---

## 5. CSWSH /ws 同源防護 + 單一寫者

[backend/internal/wsserver](../backend/internal/wsserver)：

- **同源檢查**：用 `websocket.Upgrader{}`（不覆寫 `CheckOrigin`），gorilla 預設要求
  Origin host == 請求 Host，擋掉跨站 WebSocket 劫持（別站網頁無法用操作員的環境
  session 開我們的命令通道）。**千萬別寫 `CheckOrigin: return true`**。
- **單一寫者**：gorilla 禁止並發寫同一 conn。資料推送與命令 ack 全部走**一個
  goroutine**（透過 `acks` channel），ack 用 non-blocking send（塞爆就丟，不卡讀迴圈）。
- dev proxy 副作用：`vite.config.ts` 經 dev server proxy 到後端時，Origin 是
  `localhost:5173`、Host 是後端 → 必被同源檢查拒。所以 proxy 把 `/ws` 的 Origin
  header 拔掉，走「非瀏覽器客戶端」路徑放行。正式版前端由後端同源服務，不經此路。

---

## 6. 前端 i18n（自寫 rune store）

[frontend/src/lib/i18n](../frontend/src/lib/i18n) — 不上 svelte-i18n：固定字串集的 kiosk
HMI，零依賴、無 async 載入閃爍（FOUC）。

- **資料結構**：`messages[locale][namespace][key]`，**一個畫面一個 namespace 檔**
  （互不踩線、可並行編輯）。加 namespace 只動兩處：新建檔 + 在 `messages/index.ts`
  的 `NS` 表加一行。
- `t('nav.logout', { role })` 走 dot-path 查詢 + `{name}` 佔位替換。
- **反應性陷阱**：`t()` 直接讀 `$state` 的 `locale`，所以在 template / `$derived` 裡
  呼叫即具反應性，切語言全畫面即時更新。但**把 `t()` 拿去算 array/object 必須包在
  `$derived`**（如 App.svelte 的 `roleLabel`），否則只算一次、切語言不重譯。
- 範本只留 `common`/`nav`/`login`/`demo` 四個機種無關 namespace。

---

## 7. 部署

更長版見 `.claude/skills/deploy-strategy`、`wsl-deploy-strategy`。重點雷：

- **兩條路**：`make deploy`（scp + ssh 到工業電腦）與 `make wsl-deploy`（本機 WSL
  fallback，免 scp，repo 已在 `/mnt/c` 可見）。目標用 `.env`（`PLC_HOST`/`PLC_USER`/
  `WSL_DISTRO`）覆寫，別 hardcode。
- **shm 權限雷**：CODESYS 在 WSL 以 root 跑，建出 `/dev/shm/plc_{data,cmd}` 是
  `root:root 0750`，bridge 的 service user 讀不到 → 主控顯示「PLC 未連線」。systemd
  unit 用 `ExecStartPre=+...chmod o+r/o+rw` 在 bridge 開檔前放寬（每次啟動都重做，
  因 CODESYS 每次 runtime 重啟都重建 segment）。`make wsl-bootstrap` 一次建好
  user / unit / sudoers allowlist。
- **layout 改版後**：`make shm-reset`（或 `wsl-shm-reset`）清掉 `/dev/shm/plc_*` 再
  重啟，否則舊 segment 版號對不上掛不起來。

---

## 8. 測試

- **Go**：`go test ./backend/...`——shm seqlock 契約、auth、wsserver 的 `applyCmd`
  三條路徑（已知型別 ok / 未知型別錯 / nil-sink 不 panic）。`make test` / `make vet`
  原生在 dev box 跑（shm mmap 躲在 `//go:build linux` 後，host 也能編）。
- **前端 e2e scaffold**：[frontend/playwright.config.ts](../frontend/playwright.config.ts) +
  `frontend/tests/smoke.spec.ts`（mock `/api`、`/ws`，驗證 i18n 切換）。本地用
  `npm run test:e2e`（先 `npx playwright install chromium`）。**範本刻意不把 e2e 掛進
  CI**——等長出真畫面、有值得守的東西再加回去。

---

## 9. 從這個 base 長出一台新機器

1. 從 `main` 分支（例如 `git switch -c my_machine`）。
2. **shm**：在 `layout.go` 與 `DUT/ShmBridge/*.st` 兩邊加機種欄位，**同步 bump
   `Version`**，補 `vet.go` size assert 與 layout/seqlock 測試。
3. **後端**：在 `backend/internal/` 加機種套件與 REST/WS 路由；在 plc_bridge 的
   `requiredRole` 對寫入路由設權限級。
4. **前端**：在 `messages/` 加 namespace、加畫面元件、在 App.svelte 接上。
5. **PLC**：IEC 程式碼用 [cds-text-sync](https://github.com/kevin00156/cds-text-sync) 在
   CODESYS `.project` 與 `codesys_export/*.st` 間**雙向同步**（`Project_export.py` 出、
   `Project_import.py` 回），就能用 VS Code 編、用 Git 版控（安裝與工作流見
   [README](../README.md#codesys-文字同步cds-text-sync)）。`codesys_export/` 已備有通用
   scaffold：`StateMachine`（抽象狀態機基底）、`MC_BasicControl`（PLCopen 動作包裝）、
   `Functions/`、`GlobalVars`。
6. 機種專屬的廠商參考資料（xlsx / spec / demo）放本地 `references/`（已 gitignore），
   別進範本。
