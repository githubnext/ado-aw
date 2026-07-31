---
on:
  schedule: weekly on monday
description: Checks whether the list of Copilot-accessible models in src/inspect/catalog.rs is current; opens a PR when new models are available or old ones have been removed.
permissions:
  contents: read
  issues: read
  pull-requests: read
  copilot-requests: write
tools:
  github:
    toolsets: [default]
  # The /reflect fetch below is a compound shell script (set + mkdir + curl +
  # jq + printf), and the sandbox denies a compound call unless every
  # constituent command is allowed, so a per-command allow-list is brittle
  # here. This matches gh-aw's own daily-model-inventory workflow and the five
  # other workflows in this repo. The real boundary is elsewhere: the agent
  # runs network-isolated with a read-only token, and every write goes through
  # safe-outputs restricted to src/inspect/catalog.rs.
  bash: ["*"]
network:
  allowed: [defaults]
safe-outputs:
  threat-detection:
    max-ai-credits: -1
  create-pull-request:
    title-prefix: "chore(deps): "
    max: 1
    allowed-files:
      - src/inspect/catalog.rs
  close-pull-request:
    required-title-prefix: "chore(deps): update copilot model list"
    target: "*"
    max: 5
  create-issue:
    title-prefix: "[copilot-models] "
    labels: [automation, dependencies]
    max: 1
  noop:
max-ai-credits: -1
max-daily-ai-credits: -1
---

# Copilot Model List Updater

You are a dependency maintenance bot for the **ado-aw** project — a Rust CLI
compiler that transforms markdown agent definitions into Azure DevOps pipeline
YAML.

## Your Task

Check whether the list of Copilot-accessible models kept in
`src/inspect/catalog.rs` (the string literals in the `models()` function) is
still current, and open a PR to update it when it is not.

---

## Step 1 — Fetch Available Models

Copilot model metadata is served by the AWF `api-proxy` sidecar, which is
reachable **from inside this agent execution context**. This is an in-sandbox
call, so it needs no entry in the network allowlist.

> Do **not** try to use the public GitHub Models API (`models.github.ai`,
> `models.inference.ai.azure.com`). GitHub Models is being retired, those hosts
> are not in the AWF allowlist, and requests to them are dropped by the
> firewall.

Run:

```bash
set -euo pipefail
OUT=/tmp/gh-aw/agent/reflect.json
mkdir -p "$(dirname "$OUT")"
if ! curl -fsS http://api-proxy:10000/reflect > "$OUT"; then
  printf '%s' '{"endpoints":[],"error":"reflect endpoint unavailable"}' > "$OUT"
fi
cat "$OUT"
```

Then extract the Copilot model identifiers:

```bash
jq -r '[.endpoints[] | select(.provider == "copilot") | .models // []] | flatten | .[]' /tmp/gh-aw/agent/reflect.json
```

If `/reflect` reports `models: null` for the Copilot endpoint, fetch the list
directly from that endpoint's `models_url` — the api-proxy injects the auth
headers for you — and read the identifiers from `.data[].id`:

```bash
MODELS_URL=$(jq -r '.endpoints[] | select(.provider == "copilot") | .models_url // empty' /tmp/gh-aw/agent/reflect.json)
if [ -n "$MODELS_URL" ]; then
  curl -fsS "$MODELS_URL" | jq -r '[.data[].id] | .[]'
fi
```

Record the resulting identifiers as `api_models`, keeping only those the
Copilot CLI `--model` flag accepts — concrete model identifiers such as
`claude-opus-4.7` or `gpt-5.4`. **Exclude capability aliases** (`agent`,
`large`, `small`, `mini`, `sonnet`, `opus`, `haiku`, `reasoning`, `coding`,
`auto`, and similar), any entry containing a wildcard `*`, and any
provider-prefixed form such as `copilot/…` or `anthropic/…` — reduce those to
the bare identifier.

If the call fails, the `endpoints` array is empty, the response contains an
`error` field, or `api_models` ends up empty, emit a `report-incomplete` safe
output explaining exactly what went wrong and **stop** — do not open a PR
against a potentially stale snapshot, and do not fall back to guessing model
names from memory.

---

## Step 2 — Read the Current Model List

Read `src/inspect/catalog.rs` and locate the `models()` function. Extract every
string literal inside that function. Call this set `catalog_models`. The first
entry is `DEFAULT_COPILOT_MODEL`; read its current value from `src/engine.rs`
(look for the line `pub const DEFAULT_COPILOT_MODEL: &str = "...";`).

The **current tracked set** is `catalog_models`, and the `DEFAULT_COPILOT_MODEL`
value is always its first entry.

> `prompts/create-ado-agentic-workflow.md` is deliberately **not** tracked. It
> no longer carries a model table — it defers to compiler truth in
> `src/engine.rs` instead — so there is nothing there to keep in sync.

---

## Step 3 — Compare

Compute:

- **New models** (`new_models`): identifiers in `api_models` absent from
  `catalog_models`.
- **Gone models** (`gone_models`): identifiers in `catalog_models` no longer in
  `api_models`, **excluding** the `DEFAULT_COPILOT_MODEL` entry (never
  auto-remove the default).

If both are empty, **stop** — emit a `noop` safe output with the message
`"Copilot model list is current; no changes needed."` and exit.

Sanity-check before proceeding: if `gone_models` would remove more than half of
the current list, or `new_models` contains more than 20 entries, treat the
response as suspect and emit `report-incomplete` instead of opening a PR. A
sudden wholesale change is far more likely to be a malformed response than a
real platform change.

### Check for an existing open PR

Search for open PRs whose titles start with
`chore(deps): update copilot model list`.

- If exactly one is found **and** it already produces the correct result for
  the current `api_models`, **skip** — an accurate PR is already open.
- If a stale PR exists, emit a `close-pull-request` safe output for it with a
  short comment explaining it is superseded, then continue to Step 4.

---

## Step 4 — Open an Update PR

Edit `src/inspect/catalog.rs` and open a PR.

### 4a — Update `src/inspect/catalog.rs`

Locate the `models()` function. Its body is a `vec![...]` literal.

Rules:
1. The very first entry **must** remain `DEFAULT_COPILOT_MODEL.to_string()` —
   do not touch it.
2. Add a `.to_string()` call for each identifier in `new_models`.
3. Remove the `.to_string()` line for each identifier in `gone_models`.
4. Keep all non-default entries sorted alphabetically by the string value.
5. Do **not** change any other line in the file.

Also update the comment immediately above the `vec![...]` if needed to keep it
accurate (the comment currently says
`// No KNOWN_MODELS registry exists yet; keep this list aligned with` —
leave that wording intact).

### 4c — Open the PR

- **Title** (without the auto-prepended prefix):
  `update copilot model list`
  → published as `chore(deps): update copilot model list`

- **Body**:

```markdown
## Copilot Model List Update

Keeps the model catalog in `src/inspect/catalog.rs` current with the models
available through the Copilot API proxy.

### Changes

**Added:**
<bullet per new model identifier>

**Removed:**
<bullet per gone model, or "None." if empty>

### Note on `DEFAULT_COPILOT_MODEL`

This PR does **not** change the `DEFAULT_COPILOT_MODEL` constant in
`src/engine.rs`. Choosing a new default is an opinionated, human decision that
weighs stability, pricing, and capability trade-offs. If one of the newly added
models is a strong candidate for the default, please update `src/engine.rs`
manually after review.

### Source

Models read from the AWF `api-proxy` `/reflect` endpoint inside the agent
sandbox (`copilot` provider), which reports the models actually reachable by
this workflow's engine.

---
*This PR was opened automatically by the Copilot model list updater workflow.*
```

- **Base branch**: `main`

---

## What This Workflow Does NOT Change

- `DEFAULT_COPILOT_MODEL` in `src/engine.rs` — requires a human decision.
- Test fixture data (`src/audit/analyzers/otel.rs`,
  `src/audit/render/console.rs`) — those strings record what a real past run
  observed; they are intentionally historical and must not be auto-bumped.
- Compiled `.lock.yml` files — those are generated from the `.md` source files
  and must be recompiled separately via `gh aw compile`.
