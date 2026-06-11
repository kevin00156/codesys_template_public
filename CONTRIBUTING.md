# 貢獻流程

`main` 是受保護分支。**不能直接 push 到 `main`**（對所有人生效，包含 repo 管理員）。
所有變更只能透過 **rebase 過的 Pull Request** 合併，且 CI 的 `build` job 必須綠燈。

## 工作流

```sh
# 1. 從最新的 main 開分支
git fetch origin
git switch -c feat/my-change origin/main

# 2. 改東西、commit
git add -p && git commit

# 3. push 分支、開 PR
git push -u origin feat/my-change
gh pr create --fill          # 或在 GitHub 網頁開

# 4. 合併前先 rebase 到最新 main（分支必須 up-to-date）
git fetch origin && git rebase origin/main
git push --force-with-lease

# 5. CI 綠燈後，用 "Rebase and merge" 合併（合併鈕只開放 rebase）
gh pr merge --rebase --delete-branch
```

## 受保護分支規則（`main`）

| 規則 | 效果 |
|---|---|
| Require a pull request | 禁止直接 push，一律走 PR |
| Require status checks（`build`）+ strict | CI 要綠、且分支要 rebase 到最新 main 才能合 |
| Require linear history | 不允許 merge commit（只能 rebase） |
| 合併鈕僅 rebase | squash / merge commit 已關閉 |
| Block force-push / deletion | `main` 不能被強推或刪除 |
| Apply to administrators | 規則對管理員同樣生效 |

這些設定由 [`scripts/setup-branch-protection.sh`](scripts/setup-branch-protection.sh) 一鍵套用，
也是該設定的可重現來源（GitHub 把分支保護存在伺服器端，不在 repo 內）。

## CI

[`.github/workflows/ci.yml`](.github/workflows/ci.yml) 在每個 PR 與 push 到 `main` 時跑：
前端 `npm ci && npm run build` → 後端 `go vet` / `go test` / 交叉編譯 `linux/amd64`。
前端先編，因為 Go binary 透過 `//go:embed` 吞進 `frontend/dist/`。
