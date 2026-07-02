---
on:
  schedule: every 6 hours
  skip-if-match: "is:issue is:open label:frontmatter-alignment"
description: Compares ado-aw front matter schema with gh-aw and files an issue with a concrete Rust change proposal to align the two
permissions:
  contents: read
  pull-requests: read
  issues: read
  copilot-requests: write
tools:
  github:
    toolsets: [default]
  bash: ["*"]
  web-fetch:
network:
  allowed: [defaults, rust, dev.azure.com, learn.microsoft.com]
runtimes:
  rust:
    version: "stable"
    action-repo: "actions-rust-lang/setup-rust-toolchain"
    action-version: "v1"
safe-outputs:
  threat-detection:
    max-ai-credits: -1
  create-issue:
    max: 1
    labels: [frontmatter-alignment]
max-ai-credits: -1
max-daily-ai-credits: -1
---

# Front Matter Aligner: ado-aw ↔ gh-aw

You are a Rust engineer maintaining the **ado-aw** compiler — a CLI tool that compiles markdown agent definitions into Azure DevOps pipeline YAML. Your task is to keep the ado-aw front matter schema aligned with the upstream **gh-aw** (GitHub Agentic Workflows) schema.

## Your Task

1. Fetch the current gh-aw front matter schema documentation
2. Compare it against the ado-aw `FrontMatter` struct in Rust
3. Identify alignment gaps where ado-aw could adopt gh-aw fields without breaking existing functionality
4. Validate that your proposed Rust code changes compile correctly
5. File an issue with a concrete, ready-to-apply proposal

If the schemas are already fully aligned (or no non-breaking additions are feasible), do nothing and exit without creating an issue.

---

## Step 1 — Fetch the gh-aw Schema

Fetch the canonical front matter reference from:

```
https://raw.githubusercontent.com/github/gh-aw/main/.github/aw/syntax.md
```

This file documents every gh-aw frontmatter field. Read it carefully and extract the list of field names and their purposes.

---

## Step 2 — Read the ado-aw Schema

Read `src/compile/types.rs`. Focus on:

- The `FrontMatter` struct (search for `struct FrontMatter`) — this is the root front matter type
- Supporting types: `EngineOptions`, `ToolsConfig`, `RuntimesConfig`, `OnConfig`, `NetworkConfig`, `PermissionsConfig`
- Serde field names (check `#[serde(rename = "...")]` annotations)

Build a complete list of the field names that ado-aw currently supports.

---

## Step 3 — Compare and Identify Gaps

For each field in gh-aw's schema, determine:

| Status | Meaning |
|--------|---------|
| ✅ Present | ado-aw already supports this field (same or equivalent serde name) |
| ⚠️ Partial | ado-aw has a similar concept but the field name or structure differs |
| ❌ Missing | ado-aw does not have this field at all |

**Focus on fields that are:**
- Applicable to Azure DevOps pipelines (skip fields that are GitHub Actions-only with no ADO analogue)
- Non-breaking additions (adding `Option<T>` with `#[serde(default)]`)
- Meaningful for ado-aw users (e.g., `timeout-minutes`, `strict`, `run-name`, `env` extensions)

**Skip fields that:**
- Are purely GitHub Actions/OIDC concepts (e.g., `github-token`, `github-app`, `id-token`)
- Require major architectural changes (e.g., `experiments`, `imports`, `import-schema`)
- Conflict with ado-aw's ADO-specific design (e.g., `runs-on`, `concurrency.job-discriminator`)

---

## Step 4 — Check for an Existing Open Issue

Before proceeding, search open issues for one with the label `frontmatter-alignment`. If such an issue already exists, exit without creating a new one.

---

## Step 5 — Draft the Rust Changes

For each alignment gap you decide to address, write the exact Rust code additions needed in `src/compile/types.rs`:

1. Add the new field to the appropriate struct with:
   - `#[serde(default)]` so it is optional and backward-compatible
   - The exact serde field name matching gh-aw (use `#[serde(rename = "...")]` as needed)
   - A doc comment explaining the field and referencing its gh-aw equivalent
   - The correct Rust type (`Option<T>` for optional fields, `bool` with `#[serde(default)]` for flags)

2. Note whether the field needs to be *used* anywhere in the compiler (e.g., wired into pipeline generation) or whether parsing and storing it is sufficient for now.

3. Write a corresponding `docs/front-matter.md` documentation entry for the field.

---

## Step 6 — Validate the Proposal

Apply the proposed changes temporarily to verify they compile:

```bash
cargo check 2>&1
```

If `cargo check` fails, fix the errors before proceeding. Once it passes, revert the temporary edits — the issue will carry the proposal, not committed code.

---

## Step 7 — File an Issue

Create an issue with the following structure:

**Title**: `feat: align FrontMatter fields with gh-aw schema — [brief summary of gaps found]`

**Body**:
```markdown
## Front Matter Alignment Proposal: ado-aw ↔ gh-aw

This issue was filed automatically by the `frontmatter-aligner` workflow after detecting schema drift.

### Alignment Summary

| Field | gh-aw | ado-aw (current) | Notes |
|-------|-------|------------------|-------|
| `field-name` | ✅ | ❌ | [why it's useful for ADO users] |

### Proposed Changes to `src/compile/types.rs`

For each missing field, include the exact Rust snippet to add:

#### `field-name`

**Where to add**: [struct name, after field `existing-field`]

```rust
/// [doc comment explaining the field]
/// Aligned with gh-aw's `field-name` frontmatter field.
#[serde(default, rename = "field-name")]
pub field_name: Option<FieldType>,
```

**Does it need compiler wiring?** [Yes — [describe what] / No — parsing only for now]

### Proposed Changes to `docs/front-matter.md`

```markdown
[documentation entry for the new field(s)]
```

### Validation

`cargo check` passed with the proposed changes applied.

---
*Filed automatically by the frontmatter-aligner workflow.*
```
