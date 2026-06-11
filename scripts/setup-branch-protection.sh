#!/usr/bin/env bash
# Configure "PR + rebase only" protection on main.
#
# GitHub stores branch protection server-side, not in the repo, so this script
# IS the source of truth for that config. It is idempotent — re-run it any time
# to reconcile the live settings back to what is encoded here.
#
# Requires: gh CLI authenticated with the `repo` scope and admin on the repo.
# Usage:    scripts/setup-branch-protection.sh [owner/repo] [branch]
#           (defaults: current repo from `gh repo view`, branch `main`)
set -euo pipefail

REPO="${1:-$(gh repo view --json nameWithOwner -q .nameWithOwner)}"
BRANCH="${2:-main}"

echo ">> Repo: $REPO   branch: $BRANCH"

# 1. Merge button → rebase only.
#    Disabling merge-commit and squash leaves rebase as the ONLY way a PR can
#    land on the branch. delete_branch_on_merge keeps the branch list tidy.
echo ">> Setting merge methods to rebase-only…"
gh api --method PATCH "repos/$REPO" \
  -F allow_merge_commit=false \
  -F allow_squash_merge=false \
  -F allow_rebase_merge=true \
  -F allow_auto_merge=true \
  -F delete_branch_on_merge=true >/dev/null

# 2. Branch protection on $BRANCH:
#    - required_pull_request_reviews present  → a PR is required (no direct push)
#    - required_approving_review_count: 0     → solo dev can merge own green PR
#    - required_status_checks.strict: true    → branch must be up to date (rebase
#                                               onto latest $BRANCH) before merge
#    - contexts: ["build"]                    → the CI job must pass
#    - required_linear_history: true          → no merge commits
#    - allow_force_pushes / allow_deletions   → blocked
#    - enforce_admins: true                   → applies to everyone, admins too
echo ">> Applying branch protection…"
gh api --method PUT "repos/$REPO/branches/$BRANCH/protection" --input - >/dev/null <<'JSON'
{
  "required_status_checks": {
    "strict": true,
    "contexts": ["build"]
  },
  "enforce_admins": true,
  "required_pull_request_reviews": {
    "required_approving_review_count": 0,
    "dismiss_stale_reviews": false,
    "require_code_owner_reviews": false
  },
  "restrictions": null,
  "required_linear_history": true,
  "allow_force_pushes": false,
  "allow_deletions": false,
  "block_creations": false
}
JSON

echo ">> Done. Effective settings:"
gh api "repos/$REPO/branches/$BRANCH/protection" --jq '{
  pull_request_required: (.required_pull_request_reviews != null),
  required_checks: .required_status_checks.contexts,
  strict_up_to_date: .required_status_checks.strict,
  linear_history: .required_linear_history.enabled,
  force_pushes_blocked: (.allow_force_pushes.enabled | not),
  deletions_blocked: (.allow_deletions.enabled | not),
  applies_to_admins: .enforce_admins.enabled
}'
