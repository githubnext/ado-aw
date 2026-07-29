---
name: Prompt Evaluator
description: Compares ado-aw create, update, and debug prompts on relevant pull requests
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
permissions:
  contents: read
  pull-requests: read
  copilot-requests: write
network:
  allowed: [defaults, github, rust]
tools:
  bash:
    - "cat *"
    - "ls *"
steps:
  - name: Prepare prompt evaluator worktree
    env:
      PROMPT_EVAL_BASE_SHA: ${{ github.event.pull_request.base.sha }}
      PROMPT_EVAL_HEAD_SHA: ${{ github.event.pull_request.head.sha }}
    run: |
      set -euo pipefail
      test -n "$PROMPT_EVAL_BASE_SHA"
      test -n "$PROMPT_EVAL_HEAD_SHA"
      git fetch --no-tags origin "$PROMPT_EVAL_BASE_SHA"
      git fetch --no-tags origin "$PROMPT_EVAL_HEAD_SHA"

      eval_root="${RUNNER_TEMP}/prompt-eval-head"
      if [ -e "$eval_root" ]; then
        echo "Refusing to reuse existing evaluator worktree: $eval_root" >&2
        exit 1
      fi
      git worktree add --detach "$eval_root" "$PROMPT_EVAL_HEAD_SHA"
      mkdir -p /tmp/gh-aw/agent/prompt-evals/current

      {
        printf 'PROMPT_EVAL_REPO_ROOT=%s\n' "$eval_root"
        printf 'PROMPT_EVAL_BASE_SHA=%s\n' "$PROMPT_EVAL_BASE_SHA"
        printf 'PROMPT_EVAL_HEAD_SHA=%s\n' "$PROMPT_EVAL_HEAD_SHA"
        printf 'PROMPT_EVAL_OUTPUT=%s\n' "/tmp/gh-aw/agent/prompt-evals/current"
      } >> "$GITHUB_ENV"

  - name: Build ado-aw for evaluation
    run: |
      set +e
      set -uo pipefail
      cargo build --quiet \
        --manifest-path "$PROMPT_EVAL_REPO_ROOT/Cargo.toml" \
        --bin ado-aw
      status=$?
      printf 'PROMPT_EVAL_ADO_AW=%s\n' \
        "$PROMPT_EVAL_REPO_ROOT/target/debug/ado-aw" >> "$GITHUB_ENV"
      printf '%s\n' "$status" > "$PROMPT_EVAL_OUTPUT/build-exit-code.txt"
      if [ "$status" -ne 0 ]; then
        echo "::warning title=Prompt evaluator build failed::The PR scorecard may be unavailable."
      fi
      exit 0

  - name: Install compiler-pinned Copilot CLI
    env:
      GH_HOST: github.com
    run: |
      set +e
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

  - name: Run paired prompt evaluation
    env:
      COPILOT_GITHUB_TOKEN: ${{ github.token }}
    run: |
      set +e
      set -uo pipefail
      node "$PROMPT_EVAL_REPO_ROOT/scripts/prompt-evals/run.mjs" \
        --repo-root "$PROMPT_EVAL_REPO_ROOT" \
        --output "$PROMPT_EVAL_OUTPUT" \
        --config "$PROMPT_EVAL_REPO_ROOT/scripts/prompt-evals/config.json" \
        --copilot /usr/local/bin/copilot \
        --ado-aw "$PROMPT_EVAL_ADO_AW" \
        --base-sha "$PROMPT_EVAL_BASE_SHA" \
        --head-sha "$PROMPT_EVAL_HEAD_SHA" \
        --event-name "$GITHUB_EVENT_NAME" \
        --repository "$GITHUB_REPOSITORY" \
        --run-id "$GITHUB_RUN_ID" \
        --run-url "$GITHUB_SERVER_URL/$GITHUB_REPOSITORY/actions/runs/$GITHUB_RUN_ID"
      status=$?
      printf '%s\n' "$status" > "$PROMPT_EVAL_OUTPUT/runner-exit-code.txt"
      if [ "$status" -ne 0 ]; then
        echo "::warning title=Prompt evaluator sampling failed::See the uploaded manifest for details."
      fi
      exit 0

  - name: Render prompt evaluation report
    if: ${{ always() }}
    run: |
      set -euo pipefail
      node "$PROMPT_EVAL_REPO_ROOT/scripts/prompt-evals/report.mjs" \
        --scorecard "$PROMPT_EVAL_OUTPUT/scorecard.json" \
        --manifest "$PROMPT_EVAL_OUTPUT/manifest.json" \
        --output "$PROMPT_EVAL_OUTPUT/report" \
        --status-dir "$PROMPT_EVAL_OUTPUT"

  - name: Upload prompt evaluation results
    if: ${{ always() }}
    uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1
    with:
      name: prompt-eval-results
      path: /tmp/gh-aw/agent/prompt-evals/current
      if-no-files-found: warn
      retention-days: 30
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
  noop:
    report-as-issue: false
max-ai-credits: -1
max-daily-ai-credits: -1
timeout-minutes: 120
---

# Prompt Evaluation Publisher

The paired evaluation, scoring, and Markdown rendering have already completed.

Read:

```bash
cat /tmp/gh-aw/agent/prompt-evals/current/report/report.md
```

Call `add-comment` once with that body unchanged. Do not recompute scores,
reinterpret evidence, or edit the prepared Markdown.

If the report file is missing, call `noop` with a concise infrastructure-error
explanation.
