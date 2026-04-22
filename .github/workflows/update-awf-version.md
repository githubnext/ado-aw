---
on:
  schedule: daily
description: Checks for new releases of gh-aw-firewall, copilot-cli, and gh-aw-mcpg, and syncs ecosystem_domains.json from gh-aw. Opens PRs for any updates found.
permissions:
  contents: read
  issues: read
  pull-requests: read
tools:
  github:
    toolsets: [default]
network:
  allowed: [defaults]
safe-outputs:
  create-pull-request:
    max: 4
---

# Dependency Version Updater

You are a dependency maintenance bot for the **ado-aw** project — a Rust CLI compiler that transforms markdown agent definitions into Azure DevOps pipeline YAML.

## Your Task

Check whether pinned version constants in `src/compile/common.rs` are up to date with the latest releases of their upstream dependencies, and whether `src/data/ecosystem_domains.json` matches the upstream source. For each outdated item, open a PR to update it.

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

Before proceeding, also check whether a PR already exists with a title matching the expected PR title (see Step 4). If one is already open, **skip this dependency** to avoid duplicates.

### Step 4: Create an Update PR

If the latest version is newer than the current constant:

1. Edit `src/compile/common.rs` — update **only** the relevant version string literal. Do not modify anything else in the file.

2. Create a pull request:

**For AWF_VERSION:**
- **Title**: `chore: update AWF_VERSION to <latest-version>`
- **Body**:
  ```markdown
  ## Dependency Update

  Updates the pinned `AWF_VERSION` constant in `src/compile/common.rs` from `<old-version>` to `<latest-version>`.

  ### Release

  See the [gh-aw-firewall release notes](https://github.com/github/gh-aw-firewall/releases/tag/v<latest-version>) for details.

  ---
  *This PR was opened automatically by the dependency version updater workflow.*
  ```

**For COPILOT_CLI_VERSION:**
- **Title**: `chore: update COPILOT_CLI_VERSION to <latest-version>`
- **Body**:
  ```markdown
  ## Dependency Update

  Updates the pinned `COPILOT_CLI_VERSION` constant in `src/engine.rs` from `<old-version>` to `<latest-version>`.

  ### Release

  See the [copilot-cli release notes](https://github.com/github/copilot-cli/releases/tag/v<latest-version>) for details.

  ---
  *This PR was opened automatically by the dependency version updater workflow.*
  ```

**For MCPG_VERSION:**
- **Title**: `chore: update MCPG_VERSION to <latest-version>`
- **Body**:
  ```markdown
  ## Dependency Update

  Updates the pinned `MCPG_VERSION` constant in `src/compile/common.rs` from `<old-version>` to `<latest-version>`.

  ### Release

  See the [gh-aw-mcpg release notes](https://github.com/github/gh-aw-mcpg/releases/tag/v<latest-version>) for details.

  ---
  *This PR was opened automatically by the dependency version updater workflow.*
  ```

- **Base branch**: `main`

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

Before proceeding, also check whether a PR already exists with the title `chore: sync ecosystem_domains.json from gh-aw`. If one is already open, **skip** to avoid duplicates.

### Step 4: Create a Sync PR

If the merged result differs from the current local file:

1. Write the merged JSON to `src/data/ecosystem_domains.json` (preserve 2-space indentation, one key per line, trailing newline).

2. Create a pull request:

- **Title**: `chore: sync ecosystem_domains.json from gh-aw`
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
