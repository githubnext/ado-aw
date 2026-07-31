---
# Shared `pre-agent-steps` that pre-fetch the PR diff, metadata and existing
# inline review comments before any reviewer agent starts.
#
# Works for both `pull_request` events and slash-command runs on a PR, because
# the PR number is resolved from `github.event.issue.number` or
# `github.event.pull_request.number`, whichever is present.
#
# Outputs, all under /tmp/gh-aw/agent/ so they are captured in the run artifact:
#   pr-diff.patch            — unified diff, generated files excluded, capped
#   pr-meta.json             — PR metadata
#   pr-review-comments.json  — existing inline review comments (for dedup)
#   pr-data-head-sha.txt     — cache validity marker
#
# The fetch is skipped entirely when all four files exist and the cached head
# SHA still matches the PR head, which is the common case: `pr-data-prefetch.yml`
# warms the `pr-prefetch-<sha>` Actions cache in ~30-60s while the reviewer
# activation jobs are still starting up.
#
# Usage:
#   cache:
#     key: pr-prefetch-${{ github.event.pull_request.head.sha || github.event.issue.number }}
#     path: /tmp/gh-aw/agent
#     restore-keys:
#       - pr-prefetch-${{ github.event.pull_request.number || github.event.issue.number }}-
#   imports:
#     - shared/pr-diff-data-fetch.md
#
# Exclusions are ado-aw specific. Reviewing generated output is pure noise: the
# compiled `*.lock.yml` pipelines, the ncc bundles committed at
# scripts/ado-script/*.js, the schemars-driven types.gen.ts / fact-catalog.gen.json,
# and Cargo.lock are all machine-written. Drift in them is a real concern, but it
# is `review-compiler-contract`'s job to spot it from pr-meta.json's file list,
# not something to be line-commented.

pre-agent-steps:
  - name: Pre-fetch PR diff, metadata and review comments
    env:
      GH_TOKEN: ${{ github.token }}
      PR_NUMBER: ${{ github.event.issue.number || github.event.pull_request.number }}
      PR_HEAD_SHA: ${{ github.event.pull_request.head.sha }}
      EXPR_GITHUB_REPOSITORY: ${{ github.repository }}
      PR_DIFF_MAX_LINES: "3000"
    run: |
      set -euo pipefail
      mkdir -p /tmp/gh-aw/agent

      if [ -z "${PR_NUMBER:-}" ]; then
        echo "No PR number in the event payload; writing empty pre-fetch files." >&2
        : > /tmp/gh-aw/agent/pr-diff.patch
        echo '{}' > /tmp/gh-aw/agent/pr-meta.json
        echo '[]' > /tmp/gh-aw/agent/pr-review-comments.json
        rm -f /tmp/gh-aw/agent/pr-data-head-sha.txt
        exit 0
      fi

      CURRENT_HEAD_SHA="${PR_HEAD_SHA:-}"
      if [ -z "$CURRENT_HEAD_SHA" ]; then
        CURRENT_HEAD_SHA=$(gh pr view "$PR_NUMBER" --repo "$EXPR_GITHUB_REPOSITORY" --json headRefOid --jq '.headRefOid' 2>/dev/null || true)
      fi

      CACHE_HEAD_SHA=""
      if [ -f /tmp/gh-aw/agent/pr-data-head-sha.txt ]; then
        CACHE_HEAD_SHA="$(tr -d '\n' < /tmp/gh-aw/agent/pr-data-head-sha.txt)"
      fi

      # Only trust the cache when it was written for this exact head commit.
      if [ -n "$CURRENT_HEAD_SHA" ] && [ "$CURRENT_HEAD_SHA" = "$CACHE_HEAD_SHA" ] &&
         [ -f /tmp/gh-aw/agent/pr-diff.patch ] &&
         [ -f /tmp/gh-aw/agent/pr-meta.json ] &&
         [ -f /tmp/gh-aw/agent/pr-review-comments.json ]; then
        LINES=$(wc -l < /tmp/gh-aw/agent/pr-diff.patch)
        COMMENT_COUNT=$(jq 'length' /tmp/gh-aw/agent/pr-review-comments.json)
        echo "Cache hit: reusing pre-fetched PR data for head ${CURRENT_HEAD_SHA} (${LINES} diff lines, ${COMMENT_COUNT} review comments)"
        exit 0
      fi

      { gh pr diff "$PR_NUMBER" --repo "$EXPR_GITHUB_REPOSITORY" \
          --exclude '**/*.lock.yml' \
          --exclude 'scripts/ado-script/*.js' \
          --exclude 'scripts/ado-script/test-bin/**' \
          --exclude '**/*.gen.ts' \
          --exclude '**/*.gen.json' \
          --exclude '**/dist/**' \
          --exclude 'Cargo.lock' \
          || true; } | head -n "${PR_DIFF_MAX_LINES}" > /tmp/gh-aw/agent/pr-diff.patch
      LINES=$(wc -l < /tmp/gh-aw/agent/pr-diff.patch)

      gh pr view "$PR_NUMBER" \
        --repo "$EXPR_GITHUB_REPOSITORY" \
        --json number,title,body,headRefName,headRefOid,additions,deletions,changedFiles,files \
        > /tmp/gh-aw/agent/pr-meta.json

      if [ -z "$CURRENT_HEAD_SHA" ]; then
        CURRENT_HEAD_SHA="$(jq -r '.headRefOid // empty' /tmp/gh-aw/agent/pr-meta.json)"
      fi

      gh api "repos/$EXPR_GITHUB_REPOSITORY/pulls/$PR_NUMBER/comments" \
        --paginate \
        --jq '.[] | {id, path, line: (.line // .original_line), body: .body[:200], user: .user.login}' \
        2>/dev/null | jq -s '.' > /tmp/gh-aw/agent/pr-review-comments.json ||
        echo '[]' > /tmp/gh-aw/agent/pr-review-comments.json

      if [ -n "$CURRENT_HEAD_SHA" ]; then
        printf '%s\n' "$CURRENT_HEAD_SHA" > /tmp/gh-aw/agent/pr-data-head-sha.txt
      else
        rm -f /tmp/gh-aw/agent/pr-data-head-sha.txt
      fi

      COMMENT_COUNT=$(jq 'length' /tmp/gh-aw/agent/pr-review-comments.json)
      echo "Pre-fetched PR diff (${LINES} lines), metadata and ${COMMENT_COUNT} existing review comments for head ${CURRENT_HEAD_SHA:-unknown}"
---

<!--
Documentation-only body. This component contributes `pre-agent-steps` and no
prompt text; the reviewer workflows that import it describe how to use the files
it produces. See `shared/pr-review-base.md` for the shared review contract.
-->
