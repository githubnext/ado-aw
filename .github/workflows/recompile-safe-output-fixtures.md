---
on:
  bots: ["github-actions[bot]"]
  release:
    types: [published]
  workflow_dispatch:
    inputs:
      version:
        description: "ado-aw release tag to recompile against (e.g. v0.16.0). Leave blank to use the latest published release."
        required: false
        type: string
description: Recompiles every tests/safe-outputs/*.lock.yml fixture using the latest released ado-aw binary, then opens a single PR if anything changed.
permissions:
  contents: read
  pull-requests: read
  issues: read
  copilot-requests: write
tools:
  github:
    toolsets: [default]
  bash: ["*"]
network:
  allowed: [defaults, github, dev.azure.com, learn.microsoft.com]
safe-outputs:
  create-pull-request:
    title-prefix: "chore(workflows): "
    max: 1
    allowed-files:
      - "tests/safe-outputs/*.lock.yml"
      - "tests/safe-outputs/**/*.lock.yml"
  close-pull-request:
    required-title-prefix: "chore(workflows): recompile safe-output fixtures"
    target: "*"
    max: 5
max-ai-credits: -1
max-daily-ai-credits: -1
---

# Recompile safe-output fixtures

You are a deterministic release-driven recompiler for the **ado-aw** project. Every fixture under `tests/safe-outputs/` is a daily smoke pipeline whose `<tool>.lock.yml` is produced by running `ado-aw compile <tool>.md`. Each released version of ado-aw bumps the embedded `version` field in the `# ado-aw-metadata: { … }` JSON marker at the top of every lock file (and may change other compiled output). When that bump is not propagated, the daily smokes drift away from the version of ado-aw that real users run.

Your goal each run is to land **at most one** focused PR that recompiles `tests/safe-outputs/*.lock.yml` against the **latest released** `ado-aw` binary. If nothing in the directory actually changes after recompilation, emit `noop` and exit.

> **Note on triggers.** This workflow is invoked by the `release` event when a new ado-aw release is published, or manually via `workflow_dispatch`. Release-please publishes the GitHub Release first; the `build` job inside the `Release` workflow then uploads the platform binaries and `checksums.txt`. By the time the `release` event fires both should be present, but Step 2 below adds a bounded retry so a slightly delayed asset upload does not cause a hard failure.

## Step 1 — Resolve the target ado-aw version

Determine the version tag to compile against, in this priority order:

1. If `workflow_dispatch` was used and the `version` input is non-empty, use it.
2. Otherwise, query the GitHub Releases API for the latest published release of `githubnext/ado-aw`:

   ```bash
   gh release view --repo githubnext/ado-aw --json tagName --jq '.tagName'
   ```

Normalize the result by stripping any leading `v`, then re-add it once so every downstream step uses the canonical `vX.Y.Z` form:

```bash
RAW="<value from above>"
BARE="${RAW#v}"
TAG="v${BARE}"
echo "Resolved ado-aw release: tag=$TAG bare=$BARE"
```

`TAG` is used in URLs and PR titles. `BARE` is used wherever a leading `v` would be wrong (e.g. comparing against the `version` field inside the lock-file metadata marker, which is stored without a `v`).

If the resolved tag is malformed (does not match `v[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.-]+)?`), abort with `missing-data` describing the bad input. Never proceed with an empty or unverified tag.

## Step 2 — Download and verify the released `ado-aw-linux-x64` binary

Release-please publishes the GitHub Release first; the `build` job inside the `Release` workflow then uploads the platform binaries and `checksums.txt`. By the time the `release` event fires both should be present, but add a bounded retry so a slightly delayed asset upload does not cause a hard failure.

```bash
set -euo pipefail
mkdir -p /tmp/gh-aw/agent/ado-aw-bin
BIN_URL="https://github.com/githubnext/ado-aw/releases/download/${TAG}/ado-aw-linux-x64"
SUM_URL="https://github.com/githubnext/ado-aw/releases/download/${TAG}/checksums.txt"

# Up to ~6 minutes of bounded retry (12 attempts × 30s) covers asset-upload latency.
for attempt in $(seq 1 12); do
  if curl -fsSL -o /tmp/gh-aw/agent/ado-aw-bin/ado-aw "$BIN_URL" \
       && curl -fsSL -o /tmp/gh-aw/agent/ado-aw-bin/checksums.txt "$SUM_URL"; then
    break
  fi
  echo "attempt=$attempt: release assets not yet present, sleeping 30s…"
  sleep 30
done

# After the loop, both files must exist; otherwise abort hard.
test -s /tmp/gh-aw/agent/ado-aw-bin/ado-aw
test -s /tmp/gh-aw/agent/ado-aw-bin/checksums.txt
```

Verify the SHA256 strictly — parse the exact filename column, not a loose `grep`:

```bash
cd /tmp/gh-aw/agent/ado-aw-bin
EXPECTED="$(awk '$2 == "ado-aw-linux-x64" || $2 == "*ado-aw-linux-x64" { print $1 }' checksums.txt | head -1)"
test -n "$EXPECTED" || { echo "no checksum entry for ado-aw-linux-x64"; exit 1; }
ACTUAL="$(sha256sum ado-aw | awk '{print $1}')"
[ "$EXPECTED" = "$ACTUAL" ] || { echo "checksum mismatch: expected=$EXPECTED actual=$ACTUAL"; exit 1; }
chmod +x ado-aw
./ado-aw --version
```

If the version printed by `./ado-aw --version` does not contain `BARE`, abort with `missing-data` describing the mismatch — you have downloaded the wrong asset.

## Step 2.5 — Pre-flight integrity check on the existing lock files

Before recompiling, run `ado-aw check` against every existing `tests/safe-outputs/*.lock.yml` using the **released** binary. The `check` subcommand recompiles each pipeline from its source `.md` and compares against the committed lock file; a non-zero exit means the on-disk lock file does **not** match what the released compiler would produce — i.e. it drifted (for example because someone recompiled with a dev build off `main` and merged that, or because a release shipped output changes that were never propagated). This is a primary signal that we need to recompile, independent of whether the source `.md` changed.

```bash
set -euo pipefail
cd "$GITHUB_WORKSPACE"
mkdir -p /tmp/gh-aw/agent
: > /tmp/gh-aw/agent/integrity-failures.txt
INTEGRITY_FAIL_COUNT=0
for lock in tests/safe-outputs/*.lock.yml; do
  if /tmp/gh-aw/agent/ado-aw-bin/ado-aw check "$lock" \
       > "/tmp/gh-aw/agent/check-$(basename "$lock").log" 2>&1; then
    echo "PASS $lock"
  else
    echo "FAIL $lock (exit $?)"
    echo "$lock" >> /tmp/gh-aw/agent/integrity-failures.txt
    INTEGRITY_FAIL_COUNT=$((INTEGRITY_FAIL_COUNT + 1))
  fi
done
echo "integrity_failures=$INTEGRITY_FAIL_COUNT"
```

Record the failure count and the failing-file list — both go in the PR body (Step 6) when a PR is opened. Do **not** abort on integrity failure here; this step is diagnostic only. Recompilation in Step 3 is what fixes the drift. If a single per-file check log shows an error other than a content mismatch (for example a missing source file, a codemod-required source, or an internal compiler error), include the relevant log excerpt in the PR body or — if recompile in Step 3 cannot succeed either — fall back to `report-incomplete` from Step 3.

## Step 3 — Recompile every fixture in `tests/safe-outputs/`

`ado-aw compile` accepts a single `.md` path or no arguments (cwd autodiscovery). It does **not** accept a directory argument — passing one silently produces `0 compiled, N skipped`, which is exactly the failure mode that took down [run 27020309715](https://github.com/githubnext/ado-aw/actions/runs/27020309715) and is tracked in [issue #867](https://github.com/githubnext/ado-aw/issues/867). Loop per-file from the repo root instead, and pass `--force` to bypass the GitHub-remote guard (required when running inside `githubnext/ado-aw` itself):

```bash
set -euo pipefail
cd "$GITHUB_WORKSPACE"
: > /tmp/gh-aw/agent/recompile.log
for md in tests/safe-outputs/*.md; do
  echo ">>> compiling $md" | tee -a /tmp/gh-aw/agent/recompile.log
  /tmp/gh-aw/agent/ado-aw-bin/ado-aw compile --force "$md" 2>&1 | tee -a /tmp/gh-aw/agent/recompile.log
done
```

If any per-file compile exits non-zero, the script aborts via `set -euo pipefail` and partial output is left on disk. Do **not** open a PR in that state — emit `report-incomplete` with the last ~80 lines of `/tmp/gh-aw/agent/recompile.log` so a maintainer can investigate. Stop.

## Step 3.5 — Post-compile sanity check

After recompiling, re-run `ado-aw check` against every lock file using the same released binary. Every check **must** now pass — if any still fails, our just-produced output disagrees with itself, which means the compile silently mis-handled something. That is a hard failure:

```bash
set -euo pipefail
cd "$GITHUB_WORKSPACE"
for lock in tests/safe-outputs/*.lock.yml; do
  /tmp/gh-aw/agent/ado-aw-bin/ado-aw check "$lock" \
    > "/tmp/gh-aw/agent/postcheck-$(basename "$lock").log" 2>&1 \
    || { echo "post-compile integrity STILL failing for $lock"; cat "/tmp/gh-aw/agent/postcheck-$(basename "$lock").log"; exit 1; }
done
echo "all lock files pass integrity check against ado-aw ${TAG}"
```

If this step fails, emit `report-incomplete` with the offending file name and the tail of its postcheck log; do **not** open a PR with broken integrity.

## Step 4 — Detect actual changes

Use `git status --porcelain` scoped to the fixture directory:

```bash
git status --porcelain -- tests/safe-outputs/ > /tmp/gh-aw/agent/recompile-status.txt
cat /tmp/gh-aw/agent/recompile-status.txt
```

If `/tmp/gh-aw/agent/recompile-status.txt` is empty **and** `INTEGRITY_FAIL_COUNT` from Step 2.5 was `0`, the fixtures already match the released version — emit `noop` with the message `"tests/safe-outputs/ already compiled against ado-aw ${TAG} and all integrity checks pass"` and stop. Do **not** open an empty PR.

If `/tmp/gh-aw/agent/recompile-status.txt` is empty **but** `INTEGRITY_FAIL_COUNT > 0`, this is contradictory — recompile produced no diff yet `check` reported drift. That should not happen in practice (a passing `check` and a no-op `compile` against the same source and binary must agree). Emit `report-incomplete` with the contents of `/tmp/gh-aw/agent/integrity-failures.txt` and the relevant `check-*.log` files so a maintainer can investigate. Stop.

If non-empty, inspect the diff briefly to make sure only `.lock.yml` files under `tests/safe-outputs/` changed:

```bash
git diff --name-only -- tests/safe-outputs/ | sort
```

If any path outside `tests/safe-outputs/` appears, or if any non-`.lock.yml` file appears, abort with `report-incomplete` — that is unexpected output from the compiler and a maintainer should look. The `allowed-files` glob in the front matter is a backstop, not a license to broaden scope.

## Step 5 — Determine the `from → to` version range for the PR body

Pick any one of the recompiled lock files (for example `tests/safe-outputs/noop.lock.yml`) and read the `# ado-aw-metadata: { … }` line at the top — both the new working-tree copy and the old `HEAD` copy:

```bash
SAMPLE="tests/safe-outputs/noop.lock.yml"
NEW_VER="$(head -n1 "$SAMPLE" | sed -n 's/.*"version":"\([^"]*\)".*/\1/p')"
OLD_VER="$(git show HEAD:"$SAMPLE" 2>/dev/null | head -n1 | sed -n 's/.*"version":"\([^"]*\)".*/\1/p')"
echo "from=${OLD_VER:-unknown} to=${NEW_VER}"
```

`NEW_VER` should equal `BARE`; if not, abort with `missing-data` — the compiler did not stamp the version you expected.

## Step 6 — Open the PR

The `safe-outputs.create-pull-request.title-prefix` is configured to `chore(workflows): `, so gh-aw will prepend it automatically. Provide the title **without** that prefix.

- **Title (provide without prefix)**: `recompile safe-output fixtures with ado-aw ${TAG}`
  - Published title: `chore(workflows): recompile safe-output fixtures with ado-aw ${TAG}`
- **Branch base**: `main`
- **Body**:

  ```markdown
  ## Recompile `tests/safe-outputs/` against ado-aw `${TAG}`

  Bumps the `version` field in every `tests/safe-outputs/*.lock.yml` metadata marker from `${OLD_VER}` to `${NEW_VER}`, picking up any compile-output changes shipped in [`ado-aw ${TAG}`](https://github.com/githubnext/ado-aw/releases/tag/${TAG}).

  ### Pre-flight integrity check

  Before recompiling, `ado-aw check` was run against every existing lock file using the released `${TAG}` binary:

  - **Integrity failures**: `${INTEGRITY_FAIL_COUNT}` of N files

  <if INTEGRITY_FAIL_COUNT > 0, include a fenced block listing the contents of `/tmp/gh-aw/agent/integrity-failures.txt`>

  ### Files updated

  <list of files from `git diff --name-only -- tests/safe-outputs/`, one per line in a fenced block>

  ### How this was produced

  - Downloaded `ado-aw-linux-x64` from the `${TAG}` release and verified its SHA256 against `checksums.txt`.
  - Ran `ado-aw check tests/safe-outputs/*.lock.yml` against the released binary to detect drift (see counts above).
  - Ran `ado-aw compile tests/safe-outputs/` from the repo root.
  - Re-ran `ado-aw check` against every regenerated lock file; all passed.
  - The `allowed-files` glob in this workflow restricts the diff to `tests/safe-outputs/**/*.lock.yml`.

  ### Reviewer checklist

  - [ ] Diff is limited to metadata markers (and any genuinely new compile output for `${TAG}`).
  - [ ] No source `.md` fixture changes leaked in.
  - [ ] Daily smoke pipelines in the AgentPlayground sandbox stay green after merge.

  ---
  *This PR was opened automatically by the `recompile-safe-output-fixtures` workflow.*
  ```

The `safe-outputs.close-pull-request` configuration on this workflow targets any open PR whose title starts with `chore(workflows): recompile safe-output fixtures`. After opening the new PR, emit one `close-pull-request` safe output per previously-open recompile PR (excluding the one you just opened) so superseded version bumps do not pile up. Each closure should include a short comment of the form `Superseded by the recompile PR for ado-aw ${TAG}.`. Do not close any PR whose title does not start with that exact prefix.

## When NOT to open a PR

- The resolved tag is missing or malformed (Step 1) — emit `missing-data`.
- Release assets never appear within the bounded retry window (Step 2) — emit `report-incomplete`.
- `ado-aw --version` does not contain `BARE` (Step 2) — emit `missing-data`.
- `ado-aw compile` fails (Step 3) — emit `report-incomplete`.
- Post-compile `ado-aw check` still fails for any lock file (Step 3.5) — emit `report-incomplete`.
- `tests/safe-outputs/` is already at `BARE` **and** pre-flight integrity reported zero failures (Step 4) — emit `noop`.
- Recompile produced no diff but pre-flight integrity reported failures (Step 4) — emit `report-incomplete`.
- Compile output touched paths outside `tests/safe-outputs/*.lock.yml` (Step 4) — emit `report-incomplete`.

Keep the PR small, mechanical, and reviewable. One release, one PR.
