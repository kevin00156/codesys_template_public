<!-- main 受保護：只能用 rebase 過的 PR 合併，且 CI `build` 必須綠燈。 -->

## 變更內容
<!-- 這個 PR 做了什麼、為什麼 -->

## 影響的層
- [ ] PLC（`codesys_export/`）
- [ ] 後端（`backend/`）
- [ ] 前端（`frontend/`）

## 檢查
- [ ] 已在 main 上 rebase（`git fetch origin && git rebase origin/main`）
- [ ] 本機 `make build`（或 `make test && make vet`）通過
- [ ] 若改了 shm layout，兩端的 `Version` 已同步 bump（見 backend/README.md）
