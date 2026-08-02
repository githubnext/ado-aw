---
# Shared `pre-agent-steps` that pre-fetch the PR diff, metadata and existing
# inline review comments before any reviewer agent starts.
#
# Works for `pull_request` events, inline slash-command runs, and centralized
# `/review` runs. The last case is the subtle one: with
# `slash_command.strategy: centralized`, the generated `agentic_commands.yml`
# router dispatches each reviewer via `workflow_dispatch`, so there is **no**
# `github.event.issue.number` or `github.event.pull_request.number` in the
# payload — the PR identity arrives only in the `aw_context` input as
# `{item_type: "pull_request", item_number: "<N>"}`. gh-aw's `github.aw.context.*`
# namespace resolves this, but it is a prompt-only virtual namespace
# (`transformAwContextExpression` runs during markdown expression extraction) and
# is not transformed inside step `env:`, so the fallback is spelled out here in
# the exact raw form the compiler emits — including the
# `github.event.client_payload.aw_context` arm it uses for `repository_dispatch`
# hops. The `item_type` guard matters because `item_number` is shared across
# entity kinds, so an issue-routed run must not populate the PR number slot.
#
# Belt and braces: if that still yields nothing, the PR is resolved from the
# checked-out branch. The router always dispatches on the PR head ref, so
# `GITHUB_REF_NAME` identifies the PR even if the context payload changes shape
# again. Only when both paths fail does the step fail loudly — writing empty
# files instead would make the reviewer report a successful "nothing to review"
# run and hide the breakage.
#
# Outputs, all under /tmp/gh-aw/agent/ so they are captured in the run artifact:
#   pr-diff.patch            — unified diff, generated files excluded, capped
#   pr-meta.json             — PR metadata
#   pr-review-comments.json  — existing inline review comments (for dedup)
#   pr-data-head-sha.txt     — cache validity marker
#
# The fetch is skipped entirely when all four files exist and the cached head
# SHA still matches the PR head, which is the common case: `pr-data-prefetch.yml`
# warms the `pr-prefetch-<pr-number>-<sha>` Actions cache in ~30-60s while the
# reviewer activation jobs are still starting up. Dispatch runs know the PR
# number but not the head SHA, so they rely on the `restore-keys` prefix to find
# the newest entry and on `pr-data-head-sha.txt` to reject it if it is stale.
#
# Usage:
#   cache:
#     key: pr-prefetch-${{ github.event.pull_request.number || fromJSON(github.event.inputs.aw_context || github.event.client_payload.aw_context || '{}').item_number }}-${{ github.event.pull_request.head.sha || github.run_id }}
#     path: /tmp/gh-aw/agent
#     restore-keys:
#       - pr-prefetch-${{ github.event.pull_request.number || fromJSON(github.event.inputs.aw_context || github.event.client_payload.aw_context || '{}').item_number }}-
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
      PR_NUMBER: ${{ github.event.issue.number || github.event.pull_request.number || (fromJSON(github.event.inputs.aw_context || github.event.client_payload.aw_context || '{}').item_type == 'pull_request' && fromJSON(github.event.inputs.aw_context || github.event.client_payload.aw_context || '{}').item_number) || '' }}
      PR_HEAD_SHA: ${{ github.event.pull_request.head.sha }}
      EXPR_GITHUB_REPOSITORY: ${{ github.repository }}
      PR_DIFF_MAX_LINES: "3000"
    run: |
      set -euo pipefail
      mkdir -p /tmp/gh-aw/agent

      if [ -z "${PR_NUMBER:-}" ]; then
        # The centralized router always dispatches on the PR head ref, so the
        # branch identifies the PR even when the context payload does not.
        BRANCH="${GITHUB_HEAD_REF:-${GITHUB_REF_NAME:-}}"
        if [ -n "$BRANCH" ]; then
          PR_NUMBER=$(gh pr list --repo "$EXPR_GITHUB_REPOSITORY" --head "$BRANCH" \
            --state open --limit 1 --json number --jq '.[0].number // empty' 2>/dev/null || true)
          if [ -n "$PR_NUMBER" ]; then
            echo "::warning::PR number missing from the event payload; resolved #${PR_NUMBER} from branch ${BRANCH}."
          fi
        fi
      fi

      if [ -z "${PR_NUMBER:-}" ]; then
        # Every consumer of this component is a PR reviewer, so an unresolved PR
        # number is always a bug — most likely the routing context changed shape.
        # Fail loudly: silently writing empty files makes the reviewer report a
        # successful "nothing to review" run and hides the breakage.
        echo "::error::Could not resolve a PR number from the event payload, the aw_context input or the checked-out branch." >&2
        echo "event_name=${GITHUB_EVENT_NAME:-unknown} ref_name=${GITHUB_REF_NAME:-unknown}" >&2
        exit 1
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
