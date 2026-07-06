# tools/trace/

Trace 錄製檔的解析工具。共用 loader 在 `trace_lib.py`，五個分析腳本各吃一個 trace 路徑 CLI arg。支援兩種格式（依內容自動判別）：

- CODESYS trace 檔（`.trace` / 解碼後 `.xml`）
- Rust motion-daemon 的 `plc-trace/1` columnar JSON —— HMI「Export recording」或 `curl 'http://<bridge>/api/trace/export?seconds=60' -o dump.json` 取得；欄位名如 `axis0.actPos`、`periodNs`（不帶軸前綴的別名指到 axis 0）

入口：`python parse_trace.py <path>` — iStep / engage / 速度尖峰 / 360° unwrap 概觀。

trace 原始檔不在 repo 內（`.gitignore` 全域排除 `*.trace`），自行從機台抓下後傳路徑。
