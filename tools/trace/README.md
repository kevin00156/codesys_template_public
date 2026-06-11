# tools/trace/

CODESYS trace 檔（`.trace` / 解碼後 `.xml`）的解析工具。共用 loader 在 `trace_lib.py`，五個分析腳本各吃一個 trace 路徑 CLI arg。

入口：`python parse_trace.py <path>.trace` — iStep / engage / 速度尖峰 / 360° unwrap 概觀。

trace 原始檔不在 repo 內（`.gitignore` 全域排除 `*.trace`），自行從 CODESYS Edge 抓下後傳路徑。
