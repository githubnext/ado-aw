---
on:
  schedule: weekly on monday
description: Checks whether the list of Copilot-accessible models in src/inspect/catalog.rs and prompts/create-ado-agentic-workflow.md is current; opens a PR when new models are available or old ones have been removed.
permissions:
  contents: read
  issues: read
  pull-requests: read
  copilot-requests: write
tools:
  github:
    toolsets: [default]
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
      - prompts/create-ado-agentic-workflow.md
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

Check whether the list of Copilot-accessible models kept in two files is still
current, and open a PR to update it when it is not.

The two files you are responsible for:

| File | What to update |
|------|----------------|
| `src/inspect/catalog.rs` | string literals in the `models()` function |
| `prompts/create-ado-agentic-workflow.md` | model table in "Step 2 — Engine" |

---

## Step 1 — Fetch Available Models

Use the GitHub API to list models that are accessible to GitHub Copilot:

```
GET /models
```

From the response, collect every model whose `vendor` (or `publisher`) is
`anthropic` or `openai` **and** whose identifier the Copilot CLI `--model` flag
accepts. The canonical identifier is the `id` (or `name`) field returned by the
API. Record the full list as `api_models`.

If the API call fails or returns an empty list, emit a `report-incomplete`
safe output explaining what went wrong and stop — do not open a PR against a
potentially stale snapshot.

---

## Step 2 — Read the Current Model List

**From `src/inspect/catalog.rs`:**

Read the file and locate the `models()` function. Extract every string literal
inside that function. Call this set `catalog_models`. The first entry is the
`DEFAULT_COPILOT_MODEL` (currently `"claude-opus-4.7"`).

**From `prompts/create-ado-agentic-workflow.md`:**

Read the file and locate the model table inside "Step 2 — Engine". Extract
every model identifier from the table rows. Call this set `prompt_models`.

The **current tracked set** is the union of `catalog_models` and
`prompt_models`. The `DEFAULT_COPILOT_MODEL` string is always present in
`catalog_models` as the first entry.

---

## Step 3 — Compare

Compute:

- **New models** (`new_models`): identifiers in `api_models` that are absent
  from both `catalog_models` and `prompt_models`.
- **Gone models** (`gone_models`): identifiers in the current tracked set that
  are no longer in `api_models`, **excluding** the `DEFAULT_COPILOT_MODEL`
  entry (never auto-remove the default).

If both `new_models` and `gone_models` are empty, **stop** — everything is up
to date. Emit a `noop` safe output with the message
`"Copilot model list is current; no changes needed."` and exit.

### Check for an existing open PR

Search for open PRs whose titles start with
`chore(deps): update copilot model list`.

- If exactly one such PR is found **and** it would still produce the correct
  result given the current `api_models` response (i.e. the diff it contains
  already adds all `new_models` and removes all `gone_models`), **skip** —
  an accurate PR is already open.
- If a stale PR exists (the model list has changed since it was opened), emit a
  `close-pull-request` safe output for it with a short comment explaining it is
  superseded, then proceed to Step 4 to open a fresh one.

---

## Step 4 — Open an Update PR

Edit exactly the two files described below and then open a PR.

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

### 4b — Update `prompts/create-ado-agentic-workflow.md`

Locate the model table inside "Step 2 — Engine" (search for the heading
`### Step 2 — Engine` or the table that contains `claude-opus-4.7` and
`claude-sonnet-4.6`).

Rules:
1. **Never** remove the row for the default model (`claude-opus-4.7` or
   whatever `DEFAULT_COPILOT_MODEL` is set to).
2. Add one row per model in `new_models`. Use the following guidance for the
   "Use when" column:
   - If the model name contains `opus` or `o1` or `o3`: "Highest reasoning
     capability; use for the most complex tasks."
   - If the model name contains `sonnet` or `gpt-4`: "Faster and cheaper than
     Opus; good for moderate-complexity tasks."
   - If the model name contains `haiku` or `gpt-3`: "Fastest and cheapest;
     use for simple, well-scoped tasks."
   - Otherwise: "Available model; review capabilities before choosing."
3. Remove rows for models in `gone_models`.
4. Keep the table rows sorted so that the default model is first, and the rest
   are in alphabetical order by the model identifier column.
5. Do **not** change any other part of the file.

### 4c — Open the PR

- **Title** (without the auto-prepended prefix):
  `update copilot model list`
  → published as `chore(deps): update copilot model list`

- **Body**:

```markdown
## Copilot Model List Update

Keeps the model catalog in `src/inspect/catalog.rs` and the recommended-model
table in `prompts/create-ado-agentic-workflow.md` current with the models
available via the GitHub Copilot API.

### Changes

**Added:**
<bullet per new model — identifier and "Use when" description>

**Removed:**
<bullet per gone model, or "None." if empty>

### Note on `DEFAULT_COPILOT_MODEL`

This PR does **not** change the `DEFAULT_COPILOT_MODEL` constant in
`src/engine.rs` (currently `claude-opus-4.7`). Choosing a new default is an
opinionated, human decision that weighs stability, pricing, and capability
trade-offs. If one of the newly added models is a strong candidate for the
default, please update `src/engine.rs` and `prompts/create-ado-agentic-workflow.md`
manually after review.

### Source

Models fetched from `GET /models` (GitHub Copilot API).
See the [GitHub Models documentation](https://docs.github.com/en/github-models)
for the full list of available models.

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
