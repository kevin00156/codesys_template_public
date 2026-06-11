# lib_skill_builder

通用工具：把任何 CODESYS 函式庫的官方文件抓下來，灌進 `.claude/skills/codesys-libs/libs/<LibName>/` 的 `index.json` + `reference/<Name>.{md,json}` 結構。

> 所有 CODESYS lib reference 已統一住在單一 skill `codesys-libs` 下，按 lib 名分子目錄。新增 / 更新 lib 用這個工具。

## 工作流程（每個 lib 三步）

```bash
# 0) 一次性安裝依賴
pip install -r requirements.txt

# 1) 從 lib 的 doc root 自動建 index.json
python build_index.py \
    https://content.helpme-codesys.com/en/libs/Standard/Current/ \
    ../../.claude/skills/codesys-libs/libs/Standard

# 2) 用 index.json 拉所有 item 的內容
python fetch_all.py ../../.claude/skills/codesys-libs/libs/Standard

# 3) 人工補上 libs/Standard/INDEX.md（quick-orientation + Claude 注意事項）
#    並在 codesys-libs/INDEX.md 加一列 + 在 codesys-libs/SKILL.md 加觸發詞
```

`build_index.py` 會：
1. 抓 doc root HTML
2. 從 nav tree 找出所有 `.html` 連結（限同 lib path）
3. 每個 candidate 都檢查 H1 `Name (TYPE)`：是 leaf item 才收錄
4. 從 URL path 推算 `category` / `subcategory`
5. 排序後寫入 `<skill_dir>/index.json`

`fetch_all.py` 會：
1. 讀 `<skill_dir>/index.json`
2. 並行（預設 4）抓每個 item 的 HTML
3. 透過 `extract.py` 解析成結構化 JSON + 保留原 markdown
4. 寫入 `<skill_dir>/reference/<Name>.md` 與 `<Name>.json`
5. resumable — 既存檔案會跳過，加 `--force` 強制重抓

## 檔案

| 檔案 | 用途 |
|---|---|
| `build_index.py` | 爬 lib doc root → index.json |
| `fetch_all.py` | 用 index.json 拉所有 item 內容 |
| `extract.py` | HTML/Markdown 解析器（從 sm3-basic-reference 沿用） |
| `requirements.txt` | requests + html2text + beautifulsoup4 |

## 注意事項 / 已知陷阱

- **URL 編碼**：lib 名含空格的（例如 `SysShm Implementation`），URL 用 `%20`。指令列傳 URL 時用引號包起來。
- **不是每個 lib 都有公開 doc**：CODESYS 不會把所有 system library 都發佈到 `content.helpme-codesys.com`。實測 2026-05：
  - ✅ 有公開 doc：`Standard`、`SM3_Basic`、`SM3_CamBuilder`、`SM3_CNC`、`SM3_Robotics`、`SM3_Transformation`、`IoStandard`、`SysShm Implementation`
  - ❌ 都回 403：`SysMem`、`SysTask`、`SysShmAsync`、`SysShm`（bare）、`BreakpointLogging`、`3SLicense`、`ISysTypes2`、`SysShm Interfaces`
  - 想抓沒公開的 lib，只能從 CODESYS IDE 內的 Library Repository 看 → 或人工複製貼上文件
- **FB 的 Method 是獨立頁面**：例如 `CamBuilder.Append` 在 `pou-CamBuilder/Append.html`。crawler 已處理，名字會記成 `Parent.Method`，分類 `pou-` 路徑會被去掉。
- **3-column InOut table**：Method 頁的 InOut 表頭通常是 `Scope | Name | Type`（沒 Comment）— extractor 已支援。
- **`Return` scope**：Function/Method 的回傳值在 InOut 表中標 `Return`，會放到 result["return"]。
- **`__UXINT` 寬度**：CODESYS 平台特定型別，抓下來的 `.json` 直接保留原文字串，呼叫端要自己理解（32-bit = 4 byte / 64-bit = 8 byte）。
- **Interface-only lib**：例如 `SysShm Interfaces` 整個 lib 只有 interface 定義沒有實作。仍可抓但 `inout/input/output` 大多空。
- **Visu lib**：例如 `System_VisuElem*` 內容是 visu element 屬性表，不是 IEC 程式 API；抓了大概沒用。
- **`Current/` vs 固定版本**：URL 留 `/Current/` 會跟著 CODESYS 最新版動。要固定版本，把 URL 換成 `/3.5.21.0/` 之類，再跑 `--force`。

## 已建好的 lib（在 codesys-libs skill 內）

| Lib | Items | Sub-dir |
|---|---:|---|
| `Standard` | 20 | `codesys-libs/libs/Standard/` |
| `SysShm` | 9 | `codesys-libs/libs/SysShm/` |
| `IoStandard` | 12 | `codesys-libs/libs/IoStandard/` |
| `SM3_CamBuilder` | 29 | `codesys-libs/libs/SM3_CamBuilder/` |
| `SM3_Basic` | 191 | `codesys-libs/libs/SM3_Basic/` |
| **合計** | **261** | |

## 新增一個 lib 的完整流程

1. **確認該 lib 在 CODESYS docs site 有公開**（許多 system lib 是 403）：
   ```bash
   curl -I "https://content.helpme-codesys.com/en/libs/<LibName>/Current/index.html"
   ```

2. **抓 index + 內容**：
   ```bash
   cd tools/lib_skill_builder
   python build_index.py \
       "https://content.helpme-codesys.com/en/libs/<LibName>/Current/" \
       ../../.claude/skills/codesys-libs/libs/<LibName>
   python fetch_all.py ../../.claude/skills/codesys-libs/libs/<LibName>
   ```

3. **手動寫 `libs/<LibName>/INDEX.md`** — 模板：
   ```markdown
   # <LibName> (CODESYS lib)

   一句話描述 lib 用途。

   - **Items:** N
   - **Vendor:** ...
   - **Version captured:** ...
   - **Official docs:** <url>

   ## Quick orientation

   | 區塊 | 內容 | 例子 |
   |---|---|---|
   | ... | ... | ... |

   ## Important notes for Claude

   - 列出該 lib 的型別陷阱、API 限制、常見錯誤
   - 例如 SysShm 的 `__UXINT` 寬度問題、SM3_CamBuilder 的連續性問題

   ## Rebuild

   ```bash
   python build_index.py ... && python fetch_all.py ...
   ```
   ```

4. **在 master 處更新**：
   - `codesys-libs/INDEX.md` 加一列到 catalog 表
   - `codesys-libs/SKILL.md` 的 description 加上該 lib 的觸發詞（具體的 identifier，不要寫廢話）
