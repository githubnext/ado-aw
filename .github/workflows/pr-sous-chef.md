---
name: PR Sous Chef
emoji: "👨‍🍳"
description: Keeps open non-draft PRs moving by posting targeted Copilot nudges, resolving answered review threads and pushing formatter fixes
on:
  schedule: every 15m
  workflow_dispatch:
  slash_command:
    strategy: centralized
    name: souschef
    events: [pull_request_comment]
  skip-if-no-match: "is:pr is:open -is:draft"
permissions:
  contents: read
  pull-requests: read
  issues: read
  actions: read
  copilot-requests: write
network:
  allowed: [defaults, rust, node, dev.azure.com, learn.microsoft.com]
checkout:
  fetch-depth: 0
runtimes:
  rust:
    version: "stable"
    action-repo: "actions-rust-lang/setup-rust-toolchain"
    action-version: "v1"
tools:
  github:
    toolsets: [pull_requests, repos, issues]
  bash: ["*"]
  edit:
steps:
  - name: Build the eligible PR queue
    id: fetch-prs
    env:
      GH_TOKEN: ${{ secrets.GH_AW_GITHUB_MCP_SERVER_TOKEN || secrets.GH_AW_GITHUB_TOKEN || secrets.GITHUB_TOKEN }}
      TARGET_REPOSITORY: ${{ github.repository }}
      PR_QUEUE_LIMIT: "50"
    run: |
      set -uo pipefail
      mkdir -p /tmp/gh-aw/agent

      compact_file=/tmp/gh-aw/agent/pr-sous-chef-candidates-compact.json
      raw_file=/tmp/gh-aw/agent/pr-sous-chef-raw.json
      marker='<!-- ado-aw-pr-sous-chef-nudge -->'
      cooldown_seconds=1800
      stale_check_seconds=3600

      # statusCheckRollup is requested here so the per-PR pending-check filter
      # below needs zero extra REST calls. Fetching it in this one GraphQL-backed
      # call replaces up to three REST calls per PR.
      attempt=1
      ok=0
      while [ "$attempt" -le 3 ]; do
        # `2>&1 >"$raw_file"` (in this order) sends stderr to the command
        # substitution and stdout to the file. The reverse order would send both
        # to the file, leaving $err empty and the retry classification dead.
        if err="$(gh pr list --repo "$TARGET_REPOSITORY" \
              --state open \
              --search "is:pr is:open -is:draft sort:updated-desc" \
              --limit "$PR_QUEUE_LIMIT" \
              --json number,title,url,headRefOid,headRefName,updatedAt,author,mergeStateStatus,statusCheckRollup \
              2>&1 >"$raw_file")"; then
          ok=1
          break
        fi
        if printf '%s' "$err" | grep -qiE 'HTTP 50[0234]|HTTP 429|Bad Gateway|timeout|temporarily unavailable|EOF'; then
          echo "Transient gh pr list failure (attempt ${attempt}/3): $err" >&2
          sleep 5
          attempt=$((attempt + 1))
          continue
        fi
        echo "gh pr list failed, continuing with an empty queue: $err" >&2
        break
      done

      if [ "$ok" -ne 1 ]; then
        echo '[]' > "$raw_file"
      fi

      if ! jq -e 'type == "array"' "$raw_file" >/dev/null 2>&1; then
        echo "gh pr list returned a non-array payload, continuing with an empty queue" >&2
        echo '[]' > "$raw_file"
      fi

      now_epoch=$(date -u +%s)
      : > /tmp/gh-aw/agent/eligible.ndjson

      total=0
      skipped_checks_running=0
      skipped_last_comment_from_sous_chef=0
      skipped_cooldown=0

      while IFS= read -r pr; do
        [ -n "$pr" ] || continue
        total=$((total + 1))

        number=$(jq -r '.number' <<<"$pr")
        merge_state=$(jq -r '.mergeStateStatus // ""' <<<"$pr")

        # A check that has been running for over an hour is treated as stale and
        # ignored, so a long-running agentic check cannot block nudges forever.
        pending=$(jq -r --argjson now "$now_epoch" --argjson stale "$stale_check_seconds" '
          [ .statusCheckRollup // []
            | .[]
            | select((.status // .state // "") | ascii_upcase | test("QUEUED|IN_PROGRESS|PENDING|WAITING"))
            | (.startedAt // .createdAt // null)
            | select(. != null)
            | select($now - (fromdateiso8601? // 0) < $stale)
          ] | length' <<<"$pr")

        if [ "${pending:-0}" -gt 0 ]; then
          skipped_checks_running=$((skipped_checks_running + 1))
          continue
        fi

        failed_checks=$(jq -c '
          [ .statusCheckRollup // []
            | .[]
            | select((.conclusion // .state // "") | ascii_upcase | test("FAILURE|TIMED_OUT|CANCELLED|ERROR"))
            | {name: (.name // .context // "check"), url: (.detailsUrl // .targetUrl // "")}
          ] | .[0:10]' <<<"$pr")

        comments=$(gh api "repos/$TARGET_REPOSITORY/issues/$number/comments?per_page=30" \
          --jq '[.[] | {body: .body, createdAt: .created_at}]' 2>/dev/null || echo '[]')

        # A marker comment that does not mention @copilot is informational and
        # counts neither as "already nudged" nor towards the cooldown.
        last_is_nudge=$(jq -r --arg m "$marker" '
          [ .[] | select((.body | contains($m)) and (.body | contains("@copilot"))) ] as $nudges
          | if ($nudges | length) == 0 then "false"
            elif (.[-1].body | contains($m)) and (.[-1].body | contains("@copilot")) then "true"
            else "false" end' <<<"$comments")

        # A CONFLICTING branch must still be nudged even if the last comment was
        # ours: nobody else is going to resolve the conflict.
        if [ "$last_is_nudge" = "true" ] && [ "$merge_state" != "CONFLICTING" ]; then
          skipped_last_comment_from_sous_chef=$((skipped_last_comment_from_sous_chef + 1))
          continue
        fi

        recent_nudge=$(jq -r --arg m "$marker" --argjson now "$now_epoch" --argjson cd "$cooldown_seconds" '
          [ .[]
            | select((.body | contains($m)) and (.body | contains("@copilot")))
            | select($now - (.createdAt | fromdateiso8601? // 0) < $cd)
          ] | length' <<<"$comments")

        if [ "${recent_nudge:-0}" -gt 0 ]; then
          skipped_cooldown=$((skipped_cooldown + 1))
          continue
        fi

        jq -c --argjson failed "$failed_checks" \
          '{number, title, url, headRefName, headRefOid, updatedAt,
            author: (.author.login // "unknown"),
            mergeStateStatus: (.mergeStateStatus // ""),
            failed_checks: $failed}' <<<"$pr" >> /tmp/gh-aw/agent/eligible.ndjson
      done < <(jq -c '.[]' "$raw_file")

      jq -s \
        --argjson total "$total" \
        --argjson checks "$skipped_checks_running" \
        --argjson lastc "$skipped_last_comment_from_sous_chef" \
        --argjson cool "$skipped_cooldown" \
        '{processed: $total,
          skipped_checks_running: $checks,
          skipped_last_comment_from_sous_chef: $lastc,
          skipped_cooldown: $cool,
          prs: .}' /tmp/gh-aw/agent/eligible.ndjson > "$compact_file"

      echo "Queue: $(jq -r '.prs | length' "$compact_file") eligible of ${total} open non-draft PR(s)"
      jq -r '"  skipped: checks_running=\(.skipped_checks_running) last_comment_ours=\(.skipped_last_comment_from_sous_chef) cooldown=\(.skipped_cooldown)"' "$compact_file"
safe-outputs:
  threat-detection:
    max-ai-credits: -1
  add-comment:
    max: 4
    target: "*"
  resolve-pull-request-review-thread:
    max: 40
  dismiss-pull-request-review:
    max: 20
    target: "*"
  update-pull-request:
    title: false
    body: true
    operation: append
    update-branch: true
    max: 10
    target: "*"
  push-to-pull-request-branch:
    target: "*"
    if-no-changes: ignore
    commit-title-suffix: " [pr-sous-chef]"
    # The only thing ever pushed here is `cargo fmt --all` output, so nothing
    # under .github/ should ever be touched. Excluding the whole directory --
    # not just workflows -- keeps a formatter push from being able to alter CI,
    # agentic workflow definitions or repository configuration.
    excluded-files:
      - ".github/**"
    max: 10
  create-issue:
    title-prefix: "[pr-sous-chef] "
    labels: ["automation"]
    expires: 3d
    group-by-day: true
    close-older-issues: true
  mentions:
    allowed: ["@copilot"]
  noop:
  messages:
    run-started: "🍳 [{workflow_name}]({run_url}) is preparing PRs for investigation."
    run-success: "✅ [{workflow_name}]({run_url}) finished its sous-chef pass."
    run-failure: "⚠️ [{workflow_name}]({run_url}) {status} while preparing PRs."
timeout-minutes: 25
max-ai-credits: -1
max-daily-ai-credits: -1
evals:
  - id: pr-evaluated
    question: Does the agent output confirm that it evaluated at least one open pull request for nudge eligibility?
  - id: nudge-targeted
    question: Does the agent output show a specific reason why each nudged pull request needed a nudge?
---

# PR Sous Chef 👨‍🍳

You are **pr-sous-chef**. Your job is to stop open pull requests from going
quiet — most of them authored by Copilot — by telling the author exactly what is
blocking them and clearing away the bookkeeping a human would otherwise do.

You are not a reviewer. You never judge the code. You restate what the PR's own
signals already say — failed checks, unresolved review threads, merge conflicts
— and point `@copilot` at them.

## Context

- **Repository**: ${{ github.repository }}
- **Triggered by**: ${{ github.event_name }}

## Slash-command acknowledgement (mandatory)

When this run was triggered by `/souschef` on a PR comment, you must **always**
comment on that same PR, before applying any skip logic:

1. Resolve the PR number from the event context.
2. Call `add_comment` exactly once for it.
3. The body must start with `<!-- ado-aw-pr-sous-chef-nudge -->` and briefly
   acknowledge that sous-chef was invoked.
4. Do this regardless of cooldown, pending checks or duplicate safeguards.
5. If the PR number cannot be resolved, call `report_incomplete` and explain.

On a `/souschef` run, **skip the review-dismissal step entirely** (see below) —
a manual invocation is an acknowledgement, not a licence to clean up reviews.

## Token efficiency rules (mandatory)

1. Read `/tmp/gh-aw/agent/pr-sous-chef-candidates-compact.json` first. The
   pre-filter step has already removed PRs with running checks, PRs whose latest
   comment is one of ours, and PRs still inside the 30-minute cooldown.
2. If `prs` is empty, file the run report (below) and stop. If `create_issue` is
   unavailable, fall back to `noop` with `"processed=N; nudged=0; no eligible PRs"`.
3. Process PRs in `updatedAt` descending order.
4. **Nudge at most 4 PRs per run.**
5. Prioritise, in order:
   - `mergeStateStatus == "CONFLICTING"` first;
   - PRs with unresolved review threads that already have a reply from the
     author or `@copilot`;
   - the rest by most recent `updatedAt`.
   Break ties by lower PR number, so reruns behave deterministically.
6. Use the `pr-processor` sub-agent for each PR, passing only the PR number and
   its compact entry.
7. If `pr-processor` returns non-JSON or errors, record
   `{pr_number: N, skip_reason: "sub_agent_error"}` in the report and move on.
   Do not retry.
8. Do not fetch full diffs or large file lists. You are not reviewing the code.

## Per-PR actions

For each prioritised, non-skipped PR:

### 1. Run the formatter and push if needed

- `git checkout <headRefName>`
- `cargo fmt --all`
- If `git diff --quiet` exits non-zero, call `push_to_pull_request_branch` for
  this PR.
- `git checkout -` to return.
- If `cargo fmt` is unavailable or fails, skip this step **silently** — a
  missing toolchain is not worth a comment.

Do **not** rebuild the TypeScript bundles in `scripts/ado-script/`. That needs a
full `npm ci` plus two `cargo run` invocations, which is far too expensive for a
job that runs every 15 minutes. Bundle drift is reported by the Compiler
Contract Reviewer on the PR instead.

### 2. Refresh the branch

If `mergeStateStatus` is `CONFLICTING`, **skip this step** — updating the branch
cannot succeed. Otherwise call `update_pull_request` with `update_branch: true`
and a minimal appended marker referencing `pr-sous-chef` and this run URL.

### 3. Post exactly one nudge comment

At most **one** `add_comment` per PR per run.

- The first line must be `<!-- ado-aw-pr-sous-chef-nudge -->`, and the body must
  mention `@copilot`.
- Always include `pr_number`. Never emit `add_comment` without a numeric target.
- **If `CONFLICTING`**: ask `@copilot` to merge the latest `main` into the branch
  and resolve the conflicts. Nothing else — a conflicted branch has no other
  actionable signal.
- **Otherwise**: combine everything into that single comment —
  - unresolved review threads, newest first, each with the reviewer and a direct
    link;
  - failed checks from `failed_checks`, each by name with its URL;
  - whether the branch was refreshed;
  - one clear instruction on what to do next.

Be specific. "Please address the review feedback" is useless; "CI job
`rust-tests` failed on `cargo clippy` — see <url>" is actionable.

### 4. Resolve review threads that already have a reply

For `schedule` and `workflow_dispatch` runs only, use the
`resolve_review_threads` list from `pr-processor`. Include a thread only when it
is currently unresolved, contains reviewer feedback, **and** has a later reply
from the PR author or `@copilot`.

Call `resolve_pull_request_review_thread` per thread ID. On failure, record
`{thread_id: ID, skip_reason: "resolve_failed"}` and continue.

### 5. Dismiss stale bot reviews

For `schedule` and `workflow_dispatch` runs only — **never** on a `/souschef`
run. Use the `dismiss_reviews` list from `pr-processor`, which is populated only
when **every** review thread on the PR is resolved.

Call `dismiss_pull_request_review` per review ID with the justification
`"Dismissing stale review because all PR review threads are resolved."`.

## Run summary

At the end, call `create_issue` **exactly once**. The body must begin with:

```
<!-- ado-aw-pr-sous-chef-report -->
> ⚠️ **Automated status report. Do not assign this issue to a coding agent.**
```

Then a compact table of counters — `processed`, `skipped_checks_running`,
`skipped_last_comment_from_sous_chef`, `skipped_cooldown` (all four are already
in the compact JSON), plus `nudged`, `branch_update_attempts`,
`formatter_pushes`, `conflicts_flagged`, `resolved_review_threads` and
`dismissed_reviews` from your own actions.

If any PRs were nudged, add a collapsible list of their numbers and titles.

If `create_issue` is unavailable, fall back to `noop` with a condensed
`key=value` summary.

## Formatting

Use `###` or lower for headings — never `#` or `##`. Keep the summary and
recommendations visible; wrap verbose detail in
`<details><summary>…</summary>` blocks.

## agent: `pr-processor`
---
description: Decides skip/nudge actions for a single pull request using a minimal number of API calls
model: small
---
You are given one PR number and its compact metadata. Decide what should happen
to it, using as few tool calls as possible.

Return a **single compact JSON object and nothing else**. No prose, no fences.

Fields:

- `skip_reason` — a string when the PR should be skipped, otherwise `null`.
  The caller's pre-filter already removed PRs with running checks, PRs whose
  latest comment was a sous-chef nudge, and PRs inside the cooldown window. Only
  re-check those if you have positive evidence the state changed.
- `conflicting` — `true` when `mergeStateStatus` is `CONFLICTING`.
- `attempt_branch_update` — always `false` when `conflicting` is `true`.
- `nudge_body` — one combined comment body:
  - if `conflicting`: ask `@copilot` to merge latest `main` and resolve the
    conflicts, nothing else;
  - otherwise: unresolved review threads (reviewer plus direct link, newest
    first), failed checks by name with URL, branch-refresh status, and a single
    clear next action. One comment only, never two.
- `resolve_review_threads` — array of unresolved review thread node IDs. Include
  a thread **only** when it already has a later reply from the PR author or
  `@copilot` addressing the feedback. Empty array if none.
- `dismiss_reviews` — array of review IDs. Include one **only** when the review
  was authored by a bot with state `CHANGES_REQUESTED` **and** no unresolved
  threads remain on the PR. Empty array if any thread is unresolved.

Rules:

- Make at most 8 tool calls. If that is not enough to decide confidently, set
  every field to `null` and `skip_reason` to `"insufficient_context"`.
- If you cannot determine a field, set it to `null`.
- Never include the literal marker comment text in `nudge_body`; the caller
  prepends it.
