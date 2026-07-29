---
name: Prompt Evaluator
description: Evaluates ado-aw create, update, and debug prompts on pull requests and continuously on main
on:
  pull_request:
    types: [opened, synchronize, reopened]
    paths:
      - "prompts/**"
      - "tests/prompt-evals/**"
      - "tests/prompt_contract_tests.rs"
      - "tests/prompt_eval_contract_tests.rs"
      - "scripts/prompt-evals/**"
      - ".github/workflows/prompt-evaluator.md"
  schedule: daily around 02:00
  workflow_dispatch:
    inputs:
      publish:
        description: Publish a calibration Discussion
        required: false
        type: boolean
        default: false
permissions:
  contents: read
  pull-requests: read
  actions: read
  copilot-requests: write
network:
  allowed: [defaults, github, rust]
tools:
  bash:
    - "cat *"
    - "jq *"
    - "ls *"
steps:
  - name: Prepare prompt evaluator worktree
    env:
      PROMPT_EVAL_BASE_SHA: ${{ github.event.pull_request.base.sha || '' }}
      PROMPT_EVAL_HEAD_SHA: ${{ github.event.pull_request.head.sha || github.sha }}
    run: |
      set -euo pipefail
      test -n "$PROMPT_EVAL_HEAD_SHA"
      git fetch --no-tags origin "$PROMPT_EVAL_HEAD_SHA"
      if [ -n "$PROMPT_EVAL_BASE_SHA" ]; then
        git fetch --no-tags origin "$PROMPT_EVAL_BASE_SHA"
      fi

      eval_root="${RUNNER_TEMP}/prompt-eval-head"
      if [ -e "$eval_root" ]; then
        echo "Refusing to reuse existing evaluator worktree: $eval_root" >&2
        exit 1
      fi
      git worktree add --detach "$eval_root" "$PROMPT_EVAL_HEAD_SHA"
      mkdir -p /tmp/gh-aw/agent/prompt-evals/current
      mkdir -p /tmp/gh-aw/agent/prompt-evals/history

      case "$GITHUB_EVENT_NAME" in
        pull_request) mode=pr ;;
        schedule) mode=nightly ;;
        workflow_dispatch) mode=manual ;;
        *)
          echo "Unsupported event: $GITHUB_EVENT_NAME" >&2
          exit 1
          ;;
      esac

      {
        printf 'PROMPT_EVAL_REPO_ROOT=%s\n' "$eval_root"
        printf 'PROMPT_EVAL_MODE=%s\n' "$mode"
        printf 'PROMPT_EVAL_BASE_SHA=%s\n' "$PROMPT_EVAL_BASE_SHA"
        printf 'PROMPT_EVAL_HEAD_SHA=%s\n' "$PROMPT_EVAL_HEAD_SHA"
        printf 'PROMPT_EVAL_OUTPUT=%s\n' "/tmp/gh-aw/agent/prompt-evals/current"
        printf 'PROMPT_EVAL_HISTORY=%s\n' "/tmp/gh-aw/agent/prompt-evals/history"
      } >> "$GITHUB_ENV"

  - name: Build ado-aw for evaluation
    run: |
      set -uo pipefail
      cargo build --quiet \
        --manifest-path "$PROMPT_EVAL_REPO_ROOT/Cargo.toml" \
        --bin ado-aw
      status=$?
      printf 'PROMPT_EVAL_ADO_AW=%s\n' \
        "$PROMPT_EVAL_REPO_ROOT/target/debug/ado-aw" >> "$GITHUB_ENV"
      printf '%s\n' "$status" > "$PROMPT_EVAL_OUTPUT/build-exit-code.txt"
      if [ "$status" -ne 0 ]; then
        echo "::warning title=Prompt evaluator build failed::The nightly/PR scorecard may be unavailable."
      fi
      exit 0

  - name: Install compiler-pinned Copilot CLI
    env:
      GH_HOST: github.com
    run: |
      set -uo pipefail
      version="$(
        awk -F'"' \
          '/^pub const COPILOT_CLI_VERSION: &str = / { print $2; exit }' \
          "$PROMPT_EVAL_REPO_ROOT/src/engine.rs"
      )"
      if [ -n "$version" ]; then
        bash "${RUNNER_TEMP}/gh-aw/actions/install_copilot_cli.sh" "$version"
        status=$?
      else
        status=1
      fi
      if [ "$status" -eq 0 ]; then
        /usr/local/bin/copilot --version
        status=$?
      fi
      printf '%s\n' "$status" > "$PROMPT_EVAL_OUTPUT/copilot-install-exit-code.txt"
      if [ "$status" -ne 0 ]; then
        echo "::warning title=Prompt evaluator Copilot install failed::Subject and judge runs could not start."
      fi
      exit 0

  - name: Download nightly prompt evaluation history
    if: ${{ github.event_name != 'pull_request' }}
    env:
      GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
    run: |
      set -uo pipefail
      node "$PROMPT_EVAL_REPO_ROOT/scripts/prompt-evals/fetch-history.mjs" \
        --output "$PROMPT_EVAL_HISTORY" \
        --workflow prompt-evaluator.lock.yml \
        --limit 30 \
        --repository "$GITHUB_REPOSITORY" \
        --current-run-id "$GITHUB_RUN_ID"
      status=$?
      printf '%s\n' "$status" > "$PROMPT_EVAL_OUTPUT/history-exit-code.txt"
      if [ "$status" -ne 0 ]; then
        echo "::warning title=Prompt evaluator history unavailable::Trend alerting is disabled for this run."
      fi
      exit 0

  - name: Run prompt evaluation samples
    env:
      COPILOT_GITHUB_TOKEN: ${{ github.token }}
    run: |
      set -uo pipefail
      args=(
        --mode "$PROMPT_EVAL_MODE"
        --repo-root "$PROMPT_EVAL_REPO_ROOT"
        --output "$PROMPT_EVAL_OUTPUT"
        --config "$PROMPT_EVAL_REPO_ROOT/scripts/prompt-evals/config.json"
        --copilot /usr/local/bin/copilot
        --ado-aw "$PROMPT_EVAL_ADO_AW"
        --head-sha "$PROMPT_EVAL_HEAD_SHA"
        --event-name "$GITHUB_EVENT_NAME"
        --repository "$GITHUB_REPOSITORY"
        --run-id "$GITHUB_RUN_ID"
        --run-url "$GITHUB_SERVER_URL/$GITHUB_REPOSITORY/actions/runs/$GITHUB_RUN_ID"
      )
      if [ "$PROMPT_EVAL_MODE" = pr ]; then
        args+=(--base-sha "$PROMPT_EVAL_BASE_SHA")
      fi

      node "$PROMPT_EVAL_REPO_ROOT/scripts/prompt-evals/run.mjs" "${args[@]}"
      status=$?
      printf '%s\n' "$status" > "$PROMPT_EVAL_OUTPUT/runner-exit-code.txt"
      if [ "$status" -ne 0 ]; then
        echo "::warning title=Prompt evaluator sampling failed::See the uploaded manifest for details."
      fi
      exit 0

  - name: Compute continuous prompt trends
    if: ${{ always() && github.event_name != 'pull_request' }}
    run: |
      set -uo pipefail
      if [ -f "$PROMPT_EVAL_OUTPUT/scorecard.json" ]; then
        node "$PROMPT_EVAL_REPO_ROOT/scripts/prompt-evals/trend.mjs" \
          --current "$PROMPT_EVAL_OUTPUT/scorecard.json" \
          --history "$PROMPT_EVAL_HISTORY" \
          --config "$PROMPT_EVAL_REPO_ROOT/scripts/prompt-evals/config.json" \
          --history-status "$PROMPT_EVAL_OUTPUT/history-exit-code.txt" \
          --output "$PROMPT_EVAL_OUTPUT/trend.json"
        status=$?
      else
        status=1
      fi
      printf '%s\n' "$status" > "$PROMPT_EVAL_OUTPUT/trend-exit-code.txt"
      if [ "$status" -ne 0 ]; then
        echo "::warning title=Prompt evaluator trend calculation failed::Continuous alerting is unavailable for this run."
      fi
      exit 0

  - name: Render prompt evaluation report
    if: ${{ always() }}
    env:
      PROMPT_EVAL_MANUAL_PUBLISH: ${{ inputs.publish || false }}
    run: |
      set -euo pipefail
      args=(
        --event "$GITHUB_EVENT_NAME"
        --scorecard "$PROMPT_EVAL_OUTPUT/scorecard.json"
        --manifest "$PROMPT_EVAL_OUTPUT/manifest.json"
        --output "$PROMPT_EVAL_OUTPUT/report"
        --status-dir "$PROMPT_EVAL_OUTPUT"
        --manual-publish "$PROMPT_EVAL_MANUAL_PUBLISH"
      )
      if [ -f "$PROMPT_EVAL_OUTPUT/trend.json" ]; then
        args+=(--trend "$PROMPT_EVAL_OUTPUT/trend.json")
      fi
      node "$PROMPT_EVAL_REPO_ROOT/scripts/prompt-evals/report.mjs" "${args[@]}"

  - name: Upload prompt evaluation results
    if: ${{ always() }}
    uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1
    with:
      name: prompt-eval-results
      path: /tmp/gh-aw/agent/prompt-evals/current
      if-no-files-found: warn
      retention-days: 90
safe-outputs:
  mentions: false
  allowed-github-references: []
  activation-comments: false
  report-failure-as-issue: false
  missing-tool:
    create-issue: false
  report-incomplete:
    create-issue: false
  threat-detection:
    max-ai-credits: -1
  add-comment:
    max: 1
    hide-older-comments: true
    issues: false
    pull-requests: true
  create-discussion:
    title-prefix: "[Prompt Evaluation] "
    category: "General"
    close-older-discussions: true
    close-older-key: "prompt-evaluation"
    fallback-to-issue: false
    max: 1
  noop:
    report-as-issue: false
max-ai-credits: -1
max-daily-ai-credits: -1
timeout-minutes: 120
---

# Prompt Evaluation Publisher

The evaluation, scoring, trend calculation, and Markdown rendering have already
completed deterministically. Your only task is to publish the prepared result.

Read:

```bash
cat /tmp/gh-aw/agent/prompt-evals/current/report/report-context.json
```

Follow its `action` exactly:

1. **`pr-comment`**
   - Read `report/report.md`.
   - Call `add-comment` once with that body unchanged.
2. **`discussion`**
   - Read `report/report-title.txt` and `report/report.md`.
   - Call `create-discussion` once with that title and body unchanged.
3. **`noop`**
   - Read `report/noop.txt`.
   - Call `noop` once with that reason.

Do not recompute scores, reinterpret evidence, edit the prepared Markdown, or
publish more than one safe output. If a required report file is missing, call
`noop` with a concise infrastructure-error explanation.
