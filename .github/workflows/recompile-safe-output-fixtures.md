---
on:
  workflow_run:
    workflows: ["Release"]
    types: [completed]
    branches: [main]
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
tools:
  github:
    toolsets: [default]
  bash: ["*"]
network:
  allowed: [defaults, dev.azure.com, learn.microsoft.com]
safe-outputs:
  create-pull-request:
    title-prefix: "chore(workflows): "
    max: 1
    allowed-files:
      - "tests/safe-outputs/**/*.lock.yml"
  close-pull-request:
    required-title-prefix: "chore(workflows): recompile safe-output fixtures"
    target: "*"
    max: 5
---

# Recompile safe-output fixtures

You are a deterministic release-driven recompiler for the **ado-aw** project. Every fixture under `tests/safe-outputs/` is a daily smoke pipeline whose `<tool>.lock.yml` is produced by running `ado-aw compile <tool>.md`. Each released version of ado-aw bumps the embedded `version` field in the `# ado-aw-metadata: { … }` JSON marker at the top of every lock file (and may change other compiled output). When that bump is not propagated, the daily smokes drift away from the version of ado-aw that real users run.

Your goal each run is to land **at most one** focused PR that recompiles `tests/safe-outputs/*.lock.yml` against the **latest released** `ado-aw` binary. If nothing in the directory actually changes after recompilation, emit `noop` and exit.

## Step 0 — Bail out early on irrelevant `workflow_run` triggers

This workflow may be triggered by `workflow_run` after the `Release` workflow completes. The Release workflow runs on every push to `main`; only successful runs that actually published a release have new artifacts.

If invoked via `workflow_run`:

```bash
TRIGGER="${GITHUB_EVENT_NAME:-unknown}"
echo "trigger=$TRIGGER"
if [ "$TRIGGER" = "workflow_run" ]; then
  CONCLUSION="$(jq -r '.workflow_run.conclusion // empty' "$GITHUB_EVENT_PATH" 2>/dev/null || echo "")"
  HEAD_BRANCH="$(jq -r '.workflow_run.head_branch // empty' "$GITHUB_EVENT_PATH" 2>/dev/null || echo "")"
  echo "release_conclusion=$CONCLUSION head_branch=$HEAD_BRANCH"
  if [ "$CONCLUSION" != "success" ] || [ "$HEAD_BRANCH" != "main" ]; then
    echo "Release run did not succeed on main; nothing to recompile."
    # Emit noop and stop. Do not proceed to any further steps.
    exit 0
  fi
fi
```

If you exit here, emit `noop` with a one-line reason naming `$CONCLUSION` and `$HEAD_BRANCH` so the run is observable, and do not perform any other steps.

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

Release-please publishes the GitHub Release first; the `build` job inside the `Release` workflow then uploads the platform binaries and `checksums.txt`. By the time `workflow_run` fires, both should be present, but add a bounded retry so a slightly delayed asset upload does not cause a hard failure.

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

## Step 3 — Recompile every fixture in `tests/safe-outputs/`

The `tests/safe-outputs/README.md` documents the idempotent recompile command. Run it under strict shell mode so a single fixture failure aborts the whole run instead of leaving partial output on disk:

```bash
set -euo pipefail
cd "$GITHUB_WORKSPACE"
/tmp/gh-aw/agent/ado-aw-bin/ado-aw compile tests/safe-outputs/ 2>&1 | tee /tmp/gh-aw/agent/recompile.log
```

If the compile exits non-zero, do **not** open a PR with partial output. Emit `report-incomplete` with the last ~80 lines of `/tmp/gh-aw/agent/recompile.log` so a maintainer can investigate. Stop.

## Step 4 — Detect actual changes

Use `git status --porcelain` scoped to the fixture directory:

```bash
git status --porcelain -- tests/safe-outputs/ > /tmp/gh-aw/agent/recompile-status.txt
cat /tmp/gh-aw/agent/recompile-status.txt
```

If `/tmp/gh-aw/agent/recompile-status.txt` is empty, the fixtures already match the released version — emit `noop` with the message `"tests/safe-outputs/ already compiled against ado-aw ${TAG}"` and stop. Do **not** open an empty PR.

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

  ### Files updated

  <list of files from `git diff --name-only -- tests/safe-outputs/`, one per line in a fenced block>

  ### How this was produced

  - Downloaded `ado-aw-linux-x64` from the `${TAG}` release and verified its SHA256 against `checksums.txt`.
  - Ran `ado-aw compile tests/safe-outputs/` from the repo root.
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

- `workflow_run` fired for a Release-workflow run that did not succeed on `main` (Step 0).
- The resolved tag is missing or malformed (Step 1) — emit `missing-data`.
- Release assets never appear within the bounded retry window (Step 2) — emit `report-incomplete`.
- `ado-aw --version` does not contain `BARE` (Step 2) — emit `missing-data`.
- `ado-aw compile` fails (Step 3) — emit `report-incomplete`.
- `tests/safe-outputs/` is already at `BARE` (Step 4) — emit `noop`.
- Compile output touched paths outside `tests/safe-outputs/*.lock.yml` (Step 4) — emit `report-incomplete`.

Keep the PR small, mechanical, and reviewable. One release, one PR.
