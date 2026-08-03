---
# Shared base configuration for ado-aw pull request review workflows.
#
# Bundles the tooling, network allowlist and safe-outputs that every reviewer in
# the `/review` fan-out needs, so the individual reviewers only have to declare
# their triggers, their `paths` filter and their prompt.
#
# Usage:
#   imports:
#     - uses: shared/pr-review-base.md
#       with:
#         min-integrity: approved   # optional, defaults to "approved"
#
# Every reviewer that imports this posts **inline line comments** via
# `create-pull-request-review-comment` and batches them into a **single**
# `submit-pull-request-review`. `allowed-events` deliberately omits `APPROVE`:
# the GitHub Actions actor backing `GITHUB_TOKEN` is not permitted to approve a
# pull request, so allowing it would only produce runtime failures.

import-schema:
  min-integrity:
    type: string
    default: "approved"
    description: "Minimum integrity level required for GitHub tool access"

permissions:
  contents: read
  pull-requests: read
  issues: read
  copilot-requests: write

network:
  allowed: [defaults, rust, node, dev.azure.com, learn.microsoft.com]

tools:
  github:
    min-integrity: ${{ github.aw.import-inputs.min-integrity }}
    toolsets: [pull_requests, repos]

safe-outputs:
  threat-detection:
    max-ai-credits: -1
  create-pull-request-review-comment:
    side: "RIGHT"
    max: 10
  submit-pull-request-review:
    max: 1
    allowed-events: [COMMENT, REQUEST_CHANGES]
    supersede-older-reviews: true
  noop:

max-ai-credits: -1
max-daily-ai-credits: -1
timeout-minutes: 15
---

## Shared review contract

Every reviewer built on this base follows the same rules. They are repeated in
each reviewer prompt only where a specialism needs to sharpen them.

### Read the pre-fetched data — do not call the API for it

The PR diff, metadata and existing review comments are already on disk and
cached (see `shared/pr-diff-data-fetch.md`):

| File | Content |
|---|---|
| `/tmp/gh-aw/agent/pr-diff.patch` | Complete unified diff, generated/lock/bundle files excluded |
| `/tmp/gh-aw/agent/pr-meta.json` | `number, title, body, headRefName, headRefOid, additions, deletions, changedFiles, files` |
| `/tmp/gh-aw/agent/pr-review-comments.json` | Existing inline comments as `{id, path, line, body, user}` |

**Do not** call `get_diff` or `get_review_comments` — the pre-fetched files are
complete, and re-fetching burns tokens for no new information.

### Comment only on changed lines

`create-pull-request-review-comment` is rejected by GitHub unless `path` and
`line` land inside a hunk of the current diff. Take every line number from
`pr-diff.patch`. A finding about unchanged code is **not** postable — drop it,
or if it is genuinely important, raise it in the overall review body instead.

### Never repeat yourself

Read `pr-review-comments.json` **before** posting. If an existing comment
already covers the same issue on the same file, do not post it again — this
workflow re-runs on every push, and duplicated feedback is worse than none.

### Submit exactly one review

Batch your inline comments, then call `submit-pull-request-review` once:

- `REQUEST_CHANGES` when at least one finding is genuinely merge-blocking.
- `COMMENT` when every finding is advisory.

Never attempt `APPROVE` — it is not permitted and the call will fail.

### Signal over volume

Do not flag anything a linter, compiler or formatter already catches. Do not
flag personal style preferences. A short review with three real defects is far
more valuable than twenty speculative nitpicks.

### Formatting

Use `###` or lower for headings. Keep the visible part of each comment to one
sentence stating the issue and its impact, then put the explanation, the fix
snippet and the rationale inside a `<details><summary>💡 …</summary>` block.
