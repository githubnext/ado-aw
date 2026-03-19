---
on:
  schedule: daily
description: Checks for new releases of gh-aw-firewall and opens a PR to update the AWF_VERSION constant
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
    max: 1
---

# AWF Version Updater

You are a dependency maintenance bot for the **ado-aw** project — a Rust CLI compiler that transforms markdown agent definitions into Azure DevOps pipeline YAML.

## Your Task

Check whether the `AWF_VERSION` constant in `src/compile/common.rs` is up to date with the latest release of [gh-aw-firewall](https://github.com/github/gh-aw-firewall). If a newer version is available, open a PR to update it.

## Step 1: Get the Latest gh-aw-firewall Release

Fetch the latest release of the `github/gh-aw-firewall` repository. Record the tag name, stripping any leading `v` prefix to get the bare version number (e.g. `v0.24.0` → `0.24.0`).

## Step 2: Read the Current AWF_VERSION

Read the file `src/compile/common.rs` in this repository and find the line:

```rust
pub const AWF_VERSION: &str = "...";
```

Extract the version string from that line.

## Step 3: Compare Versions

If the current `AWF_VERSION` already matches the latest release, **do nothing and stop**. The dependency is up to date.

Before proceeding, also check whether a PR already exists with a title matching `chore: update AWF_VERSION to <latest-version>`. If one is already open, **do nothing and stop** to avoid duplicates.

## Step 4: Create an Update PR

If the latest version is newer than `AWF_VERSION`:

1. Edit `src/compile/common.rs` — update the `AWF_VERSION` string literal to the new version. Change only that single string value; do not modify anything else in the file.

2. Create a pull request with:
   - **Title**: `chore: update AWF_VERSION to <latest-version>`
   - **Body**:
     ```markdown
     ## Dependency Update

     Updates the pinned `AWF_VERSION` constant in `src/compile/common.rs` from `<old-version>` to `<latest-version>`.

     ### Release

     See the [gh-aw-firewall release notes](https://github.com/github/gh-aw-firewall/releases/tag/v<latest-version>) for details.

     ---
     *This PR was opened automatically by the AWF version updater workflow.*
     ```
   - **Base branch**: `main`
