---
on:
  schedule: daily around 11:00
description: Checks for new releases of gh-aw-firewall, copilot-cli, and gh-aw-mcpg, and syncs ecosystem_domains.json from gh-aw. Opens PRs for any updates found, and files action-item issues summarizing the upstream release notes for each dependency bump.
permissions:
  contents: read
  issues: read
  pull-requests: read
  copilot-requests: write
tools:
  github:
    toolsets: [default]
network:
  allowed: [defaults, dev.azure.com, learn.microsoft.com]
safe-outputs:
  threat-detection:
    max-ai-credits: -1
  create-pull-request:
    title-prefix: "chore(deps): "
    max: 4
  close-pull-request:
    required-title-prefix: "chore(deps): "
    target: "*"
    max: 10
  create-issue:
    title-prefix: "[deps-release-notes] "
    labels: [automation, dependencies]
    max: 3
  add-comment:
    target: "*"
    required-title-prefix: "[deps-release-notes] "
    max: 3
  close-issue:
    target: "*"
    required-title-prefix: "[deps-release-notes] "
    max: 10
    state-reason: "not_planned"
max-ai-credits: -1
max-daily-ai-credits: -1
---

# Dependency Version Updater

You are a dependency maintenance bot for the **ado-aw** project — a Rust CLI compiler that transforms markdown agent definitions into Azure DevOps pipeline YAML.

## Your Task

Check whether pinned version constants in `src/compile/common.rs` are up to date with the latest releases of their upstream dependencies, and whether `src/data/ecosystem_domains.json` matches the upstream source. For each outdated item, open a PR to update it. In addition, for each of the three pinned **version constants** (not the JSON sync), analyze the upstream release notes between the previously pinned version and the new latest version, and — when the notes contain breaking changes, security fixes, notable adoptable features, or deprecations — file a companion GitHub issue summarizing the action items for ado-aw maintainers.

There are four items to check:

| Item | Upstream Source | Local Path |
|------|---------------|------------|
| `AWF_VERSION` | [github/gh-aw-firewall](https://github.com/github/gh-aw-firewall) latest release | `src/compile/common.rs` |
| `COPILOT_CLI_VERSION` | [github/copilot-cli](https://github.com/github/copilot-cli) latest release | `src/engine.rs` |
| `MCPG_VERSION` | [github/gh-aw-mcpg](https://github.com/github/gh-aw-mcpg) latest release | `src/compile/common.rs` |
| `ecosystem_domains.json` | [github/gh-aw](https://github.com/github/gh-aw) `pkg/workflow/data/ecosystem_domains.json` on `main` | `src/data/ecosystem_domains.json` |

Run the following steps **independently for each item**. One may be up to date while another is not.

---

## For AWF_VERSION, COPILOT_CLI_VERSION, MCPG_VERSION:

### Step 1: Get the Latest Release

Fetch the latest release of the upstream repository. Record the tag name, stripping any leading `v` prefix to get the bare version number (e.g. `v0.24.0` → `0.24.0`).

### Step 2: Read the Current Version

Read the file `src/compile/common.rs` (for `AWF_VERSION`, `MCPG_VERSION`) or `src/engine.rs` (for `COPILOT_CLI_VERSION`) in this repository and find the corresponding constant:

- `pub const AWF_VERSION: &str = "...";`
- `pub const COPILOT_CLI_VERSION: &str = "...";`
- `pub const MCPG_VERSION: &str = "...";`

Extract the version string.

### Step 3: Compare Versions

If the current constant already matches the latest release, **skip this dependency** — it is up to date.

Before proceeding, search for any open PRs whose titles start with the item-specific prefix listed below (see Step 4 for the per-item prefix). For each match:

- If the PR's title matches the **expected** title for the latest version exactly, **skip this dependency** — an up-to-date PR is already open.
- Otherwise the PR is an **outdated** version-bump PR for the same constant. Emit a `close-pull-request` safe output for its PR number with a short comment explaining that it is superseded by a newer version bump. Then continue to Step 4 to open the fresh PR.

Item-specific title prefixes (used to identify older outdated PRs for the same constant):

- `AWF_VERSION` → `chore(deps): update AWF_VERSION to `
- `COPILOT_CLI_VERSION` → `chore(deps): update COPILOT_CLI_VERSION to `
- `MCPG_VERSION` → `chore(deps): update MCPG_VERSION to `

Only close PRs whose titles start with one of these item-specific prefixes — never close PRs that merely share the broader `chore(deps): ` prefix but belong to a different constant.

### Step 4: Create an Update PR

If the latest version is newer than the current constant:

1. Edit `src/compile/common.rs` — update **only** the relevant version string literal. Do not modify anything else in the file.

2. Create a pull request:

The `safe-outputs.create-pull-request.title-prefix` field is configured to `chore(deps): `, so gh-aw will automatically prepend that prefix to every PR title. Provide the titles below **without** the `chore(deps): ` prefix — the compiled workflow will add it.

**For AWF_VERSION:**
- **Title**: `update AWF_VERSION to <latest-version>` (will be published as `chore(deps): update AWF_VERSION to <latest-version>`)
- **Body**:
  ```markdown
  ## Dependency Update

  Updates the pinned `AWF_VERSION` constant in `src/compile/common.rs` from `<old-version>` to `<latest-version>`.

  ### Release

  See the [gh-aw-firewall release notes](https://github.com/github/gh-aw-firewall/releases/tag/v<latest-version>) for details.

  ### Action Items

  If the upstream release notes describe changes that need follow-up in ado-aw, the workflow has recorded them on the canonical rolling issue `[deps-release-notes] awf — upstream release action items` (as a new comment for this version range). If that issue does not yet exist, it was created; if the release was routine (patch-level fixes, internal refactors, no consumer-visible effect), no action items were recorded.

  ---
  *This PR was opened automatically by the dependency version updater workflow.*
  ```

**For COPILOT_CLI_VERSION:**
- **Title**: `update COPILOT_CLI_VERSION to <latest-version>` (will be published as `chore(deps): update COPILOT_CLI_VERSION to <latest-version>`)
- **Body**:
  ```markdown
  ## Dependency Update

  Updates the pinned `COPILOT_CLI_VERSION` constant in `src/engine.rs` from `<old-version>` to `<latest-version>`.

  ### Release

  See the [copilot-cli release notes](https://github.com/github/copilot-cli/releases/tag/v<latest-version>) for details.

  ### Action Items

  If the upstream release notes describe changes that need follow-up in ado-aw, the workflow has recorded them on the canonical rolling issue `[deps-release-notes] copilot-cli — upstream release action items` (as a new comment for this version range). If that issue does not yet exist, it was created; if the release was routine (patch-level fixes, internal refactors, no consumer-visible effect), no action items were recorded.

  ---
  *This PR was opened automatically by the dependency version updater workflow.*
  ```

**For MCPG_VERSION:**
- **Title**: `update MCPG_VERSION to <latest-version>` (will be published as `chore(deps): update MCPG_VERSION to <latest-version>`)
- **Body**:
  ```markdown
  ## Dependency Update

  Updates the pinned `MCPG_VERSION` constant in `src/compile/common.rs` from `<old-version>` to `<latest-version>`.

  ### Release

  See the [gh-aw-mcpg release notes](https://github.com/github/gh-aw-mcpg/releases/tag/v<latest-version>) for details.

  ### Action Items

  If the upstream release notes describe changes that need follow-up in ado-aw, the workflow has recorded them on the canonical rolling issue `[deps-release-notes] mcpg — upstream release action items` (as a new comment for this version range). If that issue does not yet exist, it was created; if the release was routine (patch-level fixes, internal refactors, no consumer-visible effect), no action items were recorded.

  ---
  *This PR was opened automatically by the dependency version updater workflow.*
  ```

- **Base branch**: `main`

### Step 5: Record Release-Notes Action Items on the Canonical Rolling Issue (if applicable)

After emitting the version-bump PR, analyze the upstream release notes between the **current** (about-to-be-replaced) version and the **latest** version, and decide whether to record items that need follow-up in ado-aw.

Rather than opening a **new** issue per release (which floods the backlog), each dependency has a **single canonical rolling issue** titled `[deps-release-notes] <dep-token> — upstream release action items`. When there are action items for a bump, you **append a comment** to that canonical issue summarizing the new version range. You only ever **create** the issue if the canonical one does not yet exist.

This step **only** applies when Step 3 determined the version is being bumped. If the version is already up to date, skip Step 5 as well.

#### Per-dependency identifiers

Each of the three version constants uses a short dependency token in issue titles and a fixed upstream repository for release-note lookups:

| Constant | Dep token | Upstream repo for releases |
|----------|-----------|----------------------------|
| `AWF_VERSION` | `awf` | `github/gh-aw-firewall` |
| `COPILOT_CLI_VERSION` | `copilot-cli` | `github/copilot-cli` |
| `MCPG_VERSION` | `mcpg` | `github/gh-aw-mcpg` |

#### Step 5a: Fetch release notes for the version range

List the releases of the upstream repo and select every release whose tag (with leading `v` stripped) satisfies:

- strictly greater than the **current** pinned version (the value that was in the constant before this run), **and**
- less than or equal to the **latest** version selected in Step 1.

Use semantic-version comparison (compare major, then minor, then patch as integers). Pre-release tags (containing `-alpha`, `-beta`, `-rc`, etc.) are excluded.

For each selected release, capture:

- the version string (without the leading `v`)
- the release notes body
- the release URL

If the agent cannot fetch a release body for some reason, record the URL and proceed with the rest; do not abort the whole step.

#### Step 5b: Classify the changes

For each release, classify each notable bullet/section into one of these categories, using the upstream release notes' own wording. Be conservative — when in doubt, classify as "Notable" rather than "Breaking".

- **Breaking changes** — config schema changes, removed/renamed CLI flags, removed network egress, removed safe outputs, behaviour changes that an existing pinned ado-aw consumer would notice on upgrade.
- **Security fixes** — CVEs, sandbox-escape fixes, credential-handling fixes, advisory references.
- **Notable features for ado-aw to adopt** — new MCPG routing modes, new AWF egress controls, new tool surfaces, new safe outputs, observability or diagnostics features that ado-aw could plausibly surface to its users.
- **Deprecations** — fields, flags, or behaviours announced as deprecated but not yet removed.

Ignore items that are purely:

- Patch-level internal refactors with no consumer-visible effect
- Documentation-only changes upstream
- Upstream dependency bumps that do not change consumer behaviour
- CI / repo-hygiene changes upstream

#### Step 5c: Decide whether to record anything

If after classification there are **no** items across all selected releases in any of the four categories above, **skip** — do not touch the canonical issue for this dependency. The PR body's "Action Items" section then accurately reads as "the release was routine" to reviewers.

If there is **at least one** item in any category, continue to Step 5d.

#### Step 5d: Find (or create) the canonical rolling issue

Each dependency has a single canonical rolling issue titled exactly:

```
[deps-release-notes] <dep-token> — upstream release action items
```

(Note the em dash `—`, and that the title carries **no** version number.)

Search open issues whose titles start with `[deps-release-notes] <dep-token> ` (where `<dep-token>` is `awf`, `mcpg`, or `copilot-cli`). Only consider issues for **this** token — never `awf` when handling `copilot-cli`, etc.

- **If the canonical issue already exists** (title exactly `[deps-release-notes] <dep-token> — upstream release action items`): record this bump by emitting an `add_comment` safe output targeting that issue's `issue_number` (Step 5e, comment form). This is the normal steady-state path.
- **If no canonical issue exists yet** but one or more **old version-titled** issues (`[deps-release-notes] <dep-token> v<version> action items`) are open: pick the newest such issue as the canonical one going forward and `add_comment` to it (Step 5e, comment form). Do **not** create a second issue. (The `deps-notes-consolidator` workflow will retitle it to the canonical form and fold in older history on its next run.)
- **If there is no `[deps-release-notes] <dep-token> …` issue open at all**: create the canonical issue once via `create-issue` (Step 5e, create form).

Any **stray, superseded** version-titled issues for the same token that remain open (beyond the one you chose as canonical) may be closed with a `close-issue` safe output (`state_reason: not_planned`) and a short comment pointing at the canonical issue. Only close issues whose titles start with `[deps-release-notes] <dep-token> ` — never a different token or a human-authored issue.

#### Step 5e: Record the action items

**Comment form** (canonical issue already exists — the normal path). Emit an `add_comment` targeting the canonical issue's number:

```markdown
## `<dep-token>` `<old-version>` → `<latest-version>`

### Releases analyzed

- [v<version-1>](<release-url-1>)
- …
- [v<latest-version>](<release-url-latest>)

### Breaking changes

- <one bullet per breaking change, with a brief description and a link to the release that introduced it. Omit the section entirely if there are none.>

### Security fixes

- <one bullet per security fix, with a brief description and a link to the release. Omit the section entirely if there are none.>

### Notable features for ado-aw to adopt

- <one bullet per notable feature, with a brief description of how ado-aw could surface or integrate it, and a link to the release. Omit the section entirely if there are none.>

### Deprecations

- <one bullet per deprecation, with a brief description and a link to the release. Omit the section entirely if there are none.>
```

**Create form** (only when no canonical issue and no version-titled issue exists for this token). The `safe-outputs.create-issue.title-prefix` field is configured to `[deps-release-notes] `, so gh-aw automatically prepends it. Provide the title **without** the prefix.

- **Title**: `<dep-token> — upstream release action items` (will be published as `[deps-release-notes] <dep-token> — upstream release action items`)
- **Body**:
  ```markdown
  # Rolling upstream release action items — `<dep-token>`

  This is the **single canonical tracking issue** for action items arising from new releases of the `<dep-token>` dependency. The dependency version updater workflow appends a new comment to this issue for each version bump, so **the most recent activity lives in the comments below**.

  The companion version-bump PR for the first recorded range is titled `chore(deps): update <CONSTANT_NAME> to <latest-version>`.

  ## `<dep-token>` `<old-version>` → `<latest-version>`

  ### Releases analyzed

  - [v<version-1>](<release-url-1>)
  - …
  - [v<latest-version>](<release-url-latest>)

  ### Breaking changes

  - <one bullet per breaking change, with a brief description and a link to the release that introduced it. Omit the section entirely if there are none.>

  ### Security fixes

  - <one bullet per security fix, with a brief description and a link to the release. Omit the section entirely if there are none.>

  ### Notable features for ado-aw to adopt

  - <one bullet per notable feature, with a brief description of how ado-aw could surface or integrate it, and a link to the release. Omit the section entirely if there are none.>

  ### Deprecations

  - <one bullet per deprecation, with a brief description and a link to the release. Omit the section entirely if there are none.>
  ```

Keep the body/comment grounded in the actual upstream release notes — do not invent items, and do not paraphrase so heavily that the upstream wording is lost. Each bullet should be checkable against the linked release.

---


## For ecosystem_domains.json:

### Step 1: Fetch the Upstream File

Read the file `pkg/workflow/data/ecosystem_domains.json` from the `main` branch of [github/gh-aw](https://github.com/github/gh-aw).

### Step 2: Read the Local File

Read `src/data/ecosystem_domains.json` in this repository.

### Step 3: Merge and Compare

Our local file may contain **additional entries** that do not exist upstream (e.g., `"lean"`). These are ado-aw-specific additions and must be preserved.

Merge the two files as follows:
- Start with all entries from the **upstream** file (updating any existing keys to match upstream values).
- **Add back** any keys that exist in the local file but **not** in the upstream file. These are ado-aw-specific entries.
- Maintain alphabetical key ordering in the final JSON.

If the merged result is identical to the current local file, **skip** — everything is up to date.

Before proceeding, search for any open PRs whose titles start with `chore(deps): sync ecosystem_domains.json from gh-aw`. Because the title contains no version number, only one such PR should ever be open at a time:

- If exactly one such PR is already open, **skip** to avoid duplicates.
- If multiple are somehow open, emit a `close-pull-request` safe output for the **older** ones (keep the most recently updated and skip; or close them all and let Step 4 open a fresh one).

Only close PRs whose titles start with the prefix `chore(deps): sync ecosystem_domains.json from gh-aw` — never close PRs from other items.

### Step 4: Create a Sync PR

If the merged result differs from the current local file:

1. Write the merged JSON to `src/data/ecosystem_domains.json` (preserve 2-space indentation, one key per line, trailing newline).

2. Create a pull request:

- **Title**: `sync ecosystem_domains.json from gh-aw` (will be published as `chore(deps): sync ecosystem_domains.json from gh-aw`)
- **Body**:
  ```markdown
  ## Ecosystem Domains Sync

  Merges upstream changes from [`github/gh-aw/pkg/workflow/data/ecosystem_domains.json`](https://github.com/github/gh-aw/blob/main/pkg/workflow/data/ecosystem_domains.json) into `src/data/ecosystem_domains.json`.

  This sync preserves any ado-aw-specific entries (keys not present upstream) while updating all shared entries to match the upstream source.

  This file defines the domain allowlists for ecosystem identifiers (e.g., `python`, `rust`, `node`) used in the `network.allowed` front matter field.

  ---
  *This PR was opened automatically by the dependency version updater workflow.*
  ```

- **Base branch**: `main`
