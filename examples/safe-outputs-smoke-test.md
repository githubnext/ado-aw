---
name: "Safe Outputs Smoke Test"
description: "End-to-end smoke test that exercises every safe-output tool against a real ADO project. The agent follows a deterministic script — no creative reasoning required."
on:
  schedule: weekly on monday around 09:00
tools:
  azure-devops:
    org: "placeholder-org"  # operator must replace with actual ADO org name
parameters:
  # ──────────────────────────────────────────────────────────────────────────
  # REQUIRED — Operator must fill these in before the first run.
  # See the "Prerequisites" section in the agent body below for how to obtain
  # each value.
  # ──────────────────────────────────────────────────────────────────────────
  - name: smokeTestWorkItemId1
    displayName: "Work item ID #1 (for comment / update / link / attachment tests)"
    type: number
    default: 0
  - name: smokeTestWorkItemId2
    displayName: "Work item ID #2 (link-work-items target)"
    type: number
    default: 0
  - name: smokeTestPullRequestId
    displayName: "Open PR ID (for add-pr-comment / reply / resolve / review / update-pr tests)"
    type: number
    default: 0
  - name: smokeTestPrThreadId
    displayName: "Existing PR thread ID on the smoke-test PR (for reply-to-pr-comment / resolve-pr-thread)"
    type: number
    default: 0
  - name: smokeTestWikiName
    displayName: "Wiki name or GUID (for create-wiki-page / update-wiki-page tests)"
    type: string
    default: ""
  - name: smokeTestQueueBuildPipelineId
    displayName: "Pipeline definition ID to trigger (for queue-build test; set 0 to skip)"
    type: number
    default: 0
safe-outputs:
  # ── Work item tools ──────────────────────────────────────────────────────
  create-work-item:
    work-item-type: Task
    tags:
      - smoke-test
      - automated
    max: 2

  comment-on-work-item:
    target: "*"
    max: 3

  update-work-item:
    target: "*"
    status: true
    body: true
    title: true
    max: 3

  link-work-items:
    target: "*"
    allowed-link-types: [related, child]
    max: 5

  # ── Pull request tools ───────────────────────────────────────────────────
  create-pull-request:
    target-branch: main
    title-prefix: "[smoke-test] "
    draft: true
    if-no-changes: warn
    max: 1

  add-pr-comment:
    comment-prefix: "[smoke-test] "
    max: 3

  reply-to-pr-comment:
    max: 3

  resolve-pr-thread:
    allowed-statuses:
      - fixed
      - wont-fix
    max: 3

  submit-pr-review:
    allowed-events:
      - comment
      - approve-with-suggestions
    max: 2

  update-pr:
    allowed-operations:
      - add-labels
      - update-description
      - vote
    allowed-votes:
      - approve-with-suggestions
      - wait-for-author
    max: 3

  # ── Repository / build tools ─────────────────────────────────────────────
  create-branch:
    branch-pattern: "^smoke-test/.*$"
    max: 2

  create-git-tag:
    tag-pattern: "^smoke-test/.*$"
    max: 1

  add-build-tag:
    tag-prefix: "smoke-test-"
    max: 3

  queue-build:
    # REQUIRED — operator must set at least one allowed pipeline ID.
    # Leave empty ([]) to skip the queue-build test; the agent will call
    # report-incomplete for this tool when it detects the list is empty.
    allowed-pipelines: []
    default-branch: main
    max: 1

  # ── Artifact / file upload tools ─────────────────────────────────────────
  upload-pipeline-artifact:
    allowed-extensions: [".txt", ".json", ".md"]
    max: 2

  upload-build-attachment:
    allowed-extensions: [".txt", ".json", ".md"]
    max: 2

  upload-workitem-attachment:
    allowed-extensions: [".txt"]
    max: 2

  # ── Wiki tools ───────────────────────────────────────────────────────────
  create-wiki-page:
    # REQUIRED — operator must replace the placeholder below with the actual
    # wiki name or GUID (e.g. "MyProject.wiki" or "4f3a1b2c-…").
    # Run: az devops wiki list --project <project> --org <orgUrl>
    wiki-name: "PLACEHOLDER_WIKI_NAME"
    path-prefix: /smoke-test
    max: 2

  update-wiki-page:
    wiki-name: "PLACEHOLDER_WIKI_NAME"
    path-prefix: /smoke-test
    max: 2
---

## Safe Outputs Smoke Test

You are a **smoke-test agent**. Your sole purpose is to verify that every
safe-output tool in this ADO project is operational. Follow the instructions
below exactly — do not deviate, skip steps, or invent extra actions. When a
tool call succeeds, move to the next one. When a tool call fails, call
`report-incomplete` with the tool name and the error detail, then continue.

---

## Prerequisites

These resources must exist in the ADO project **before** the pipeline is first
queued. A one-time setup agent (or human operator) should create them once and
record the resulting IDs as pipeline parameter defaults.

### Work items (smokeTestWorkItemId1 and smokeTestWorkItemId2)

Create two **Task** work items in the project. They are used by
`comment-on-work-item`, `update-work-item`, `link-work-items`, and
`upload-workitem-attachment`. The IDs may be reused across runs; neither
will be deleted by the test.

```
az devops work-item create \
  --title "Smoke test fixture — do not delete" \
  --type Task \
  --description "Created by the safe-outputs smoke test." \
  --org <orgUrl> --project <project>
```

Record the `id` field from the JSON response into
`smokeTestWorkItemId1` and `smokeTestWorkItemId2` (run the command twice).

### Open pull request (smokeTestPullRequestId)

An open, non-draft PR in the **self** repository is required for:
`add-pr-comment`, `reply-to-pr-comment`, `resolve-pr-thread`,
`submit-pr-review`, and `update-pr`.

1. Create a feature branch from `main`:
   ```
   az repos ref create --name refs/heads/smoke-test/fixture \
     --object-id <main-SHA> --org <orgUrl> --project <project> \
     --repository <repoName>
   ```
2. Create the PR:
   ```
   az repos pr create --title "Smoke test fixture PR — keep open" \
     --source-branch smoke-test/fixture --target-branch main \
     --org <orgUrl> --project <project> --repository <repoName>
   ```
3. Record the `pullRequestId` from the JSON response into
   `smokeTestPullRequestId`.

### PR comment thread (smokeTestPrThreadId)

A comment thread on the smoke-test PR is required for `reply-to-pr-comment`
and `resolve-pr-thread`. Add one comment to the PR and record the thread ID:

```
az repos pr comment add --comment "Smoke test thread — do not delete" \
  --id <smokeTestPullRequestId> \
  --org <orgUrl> --project <project>
```

Record the `id` from the response (the `threadId`) into
`smokeTestPrThreadId`.

### Wiki (smokeTestWikiName)

A project or code wiki must exist for `create-wiki-page` and
`update-wiki-page`. The `/smoke-test` page prefix is reserved for the test.

```
az devops wiki list --project <project> --org <orgUrl>
```

Pick any wiki from the list and record its `name` or `id` field into
`smokeTestWikiName`. Also update the `wiki-name:` fields in this file's
front matter to the same value.

Create the initial `/smoke-test` parent page so that child pages can be
created under it (some ADO wiki APIs require the parent to exist):

```
az devops wiki page create \
  --path /smoke-test --content "# Smoke test root" \
  --wiki <wikiName> --project <project> --org <orgUrl>
```

### Pipeline definition ID (smokeTestQueueBuildPipelineId)

Identify a pipeline in the project that is safe to trigger during smoke
tests (e.g. a short health-check pipeline that exits immediately). Obtain
its numeric definition ID:

```
az pipelines list --project <project> --org <orgUrl>
```

Record the ID into `smokeTestQueueBuildPipelineId` **and** add it to the
`queue-build.allowed-pipelines:` list in this file's front matter, then
recompile.

---

## Smoke Test Execution Plan

Execute the steps below in order. For each step:
- Call the tool.
- If it succeeds, continue to the next step.
- If it fails with a configuration or permission error, call
  `report-incomplete` with the tool name and the failure message, then
  continue with the next step.

### Step 0 — Read runtime context

Read the pipeline parameters from environment variables:
- `SMOKE_WORK_ITEM_ID_1` = `$(smokeTestWorkItemId1)`
- `SMOKE_WORK_ITEM_ID_2` = `$(smokeTestWorkItemId2)`
- `SMOKE_PR_ID` = `$(smokeTestPullRequestId)`
- `SMOKE_PR_THREAD_ID` = `$(smokeTestPrThreadId)`
- `SMOKE_WIKI_NAME` = `$(smokeTestWikiName)`
- `SMOKE_QUEUE_BUILD_PIPELINE_ID` = `$(smokeTestQueueBuildPipelineId)`

Verify none are zero/empty (except `SMOKE_QUEUE_BUILD_PIPELINE_ID` which
may be 0 to skip that test). If `SMOKE_WORK_ITEM_ID_1` is 0, call
`report-incomplete` with reason "smokeTestWorkItemId1 parameter not set —
see Prerequisites section" and halt.

### Step 1 — noop

```
call noop(context="smoke-test step 1: verifying noop tool is reachable")
```

### Step 2 — create-work-item

```
call create-work-item(
  title="Smoke test ephemeral item",
  description="Created by the safe-outputs smoke test. It is safe to delete.",
  tags=["smoke-test"]
)
```

Record the returned work item ID as `NEW_WI_ID` for use in later steps.

### Step 3 — comment-on-work-item

```
call comment-on-work-item(
  work_item_id=SMOKE_WORK_ITEM_ID_1,
  body="Smoke test comment — verifying comment-on-work-item is operational."
)
```

### Step 4 — update-work-item

```
call update-work-item(
  id=NEW_WI_ID,
  title="Smoke test ephemeral item (updated)",
  body="Updated by safe-outputs smoke test. Safe to delete."
)
```

### Step 5 — link-work-items

```
call link-work-items(
  source_id=SMOKE_WORK_ITEM_ID_1,
  target_id=SMOKE_WORK_ITEM_ID_2,
  link_type="related",
  comment="Smoke test link — verifying link-work-items is operational."
)
```

### Step 6 — upload-workitem-attachment

Create a small text file `smoke-test-attachment.txt` with content
`"smoke test attachment"`, then:

```
call upload-workitem-attachment(
  work_item_id=SMOKE_WORK_ITEM_ID_1,
  file_path="smoke-test-attachment.txt",
  comment="Smoke test attachment"
)
```

### Step 7 — add-pr-comment

```
call add-pr-comment(
  pull_request_id=SMOKE_PR_ID,
  content="Smoke test comment — verifying add-pr-comment is operational."
)
```

### Step 8 — reply-to-pr-comment

```
call reply-to-pr-comment(
  pull_request_id=SMOKE_PR_ID,
  thread_id=SMOKE_PR_THREAD_ID,
  content="Smoke test reply — verifying reply-to-pr-comment is operational."
)
```

### Step 9 — submit-pr-review

```
call submit-pr-review(
  pull_request_id=SMOKE_PR_ID,
  event="comment",
  body="Smoke test review comment — verifying submit-pr-review is operational."
)
```

### Step 10 — update-pr (add labels)

```
call update-pr(
  pull_request_id=SMOKE_PR_ID,
  operation="add-labels",
  labels=["smoke-test"]
)
```

If the label `smoke-test` does not yet exist in the project, the ADO API
will create it automatically.

### Step 11 — resolve-pr-thread

```
call resolve-pr-thread(
  pull_request_id=SMOKE_PR_ID,
  thread_id=SMOKE_PR_THREAD_ID,
  status="fixed"
)
```

### Step 12 — create-branch

```
call create-branch(
  branch_name="smoke-test/ephemeral-$(Build.BuildId)",
  source_branch="main"
)
```

### Step 13 — create-git-tag

```
call create-git-tag(
  tag_name="smoke-test/$(Build.BuildId)",
  message="Smoke test tag created by build $(Build.BuildId)"
)
```

### Step 14 — create-pull-request

Create a minimal dummy file `smoke-test-pr-$(Build.BuildId).txt` with
content `"smoke test"`, then:

```
call create-pull-request(
  title="smoke test PR $(Build.BuildId)",
  description="Ephemeral PR created by the safe-outputs smoke test. Safe to abandon."
)
```

### Step 15 — add-build-tag

```
call add-build-tag(
  build_id=$(Build.BuildId),
  tag="smoke-test-passed"
)
```

### Step 16 — upload-pipeline-artifact

Create a small JSON file `smoke-test-result.json` with content
`{"status":"ok","build":"$(Build.BuildId)"}`, then:

```
call upload-pipeline-artifact(
  artifact_name="smoke-test-result",
  file_path="smoke-test-result.json"
)
```

### Step 17 — upload-build-attachment

```
call upload-build-attachment(
  artifact_name="smoke-test-attachment",
  file_path="smoke-test-result.json"
)
```

### Step 18 — queue-build

If `SMOKE_QUEUE_BUILD_PIPELINE_ID` is 0, skip this step and call
`missing-data(data_type="pipeline_definition_id", reason="smokeTestQueueBuildPipelineId parameter is 0 — set it to enable queue-build testing")`.

Otherwise:

```
call queue-build(
  pipeline_id=SMOKE_QUEUE_BUILD_PIPELINE_ID,
  branch="main",
  reason="Triggered by safe-outputs smoke test (build $(Build.BuildId))"
)
```

### Step 19 — create-wiki-page

If `SMOKE_WIKI_NAME` is empty, call
`missing-data(data_type="wiki_name", reason="smokeTestWikiName parameter is empty — set it to enable wiki testing")` and skip steps 19–20.

```
call create-wiki-page(
  path="/smoke-test/run-$(Build.BuildId)",
  content="# Smoke test run $(Build.BuildId)\n\nAll safe-output tools exercised successfully."
)
```

### Step 20 — update-wiki-page

```
call update-wiki-page(
  path="/smoke-test/run-$(Build.BuildId)",
  content="# Smoke test run $(Build.BuildId)\n\nAll safe-output tools exercised successfully. Updated at step 20."
)
```

### Step 21 — report results

If all steps above completed without calling `report-incomplete`, call:

```
call noop(context="Smoke test complete — all safe-output tools exercised successfully in build $(Build.BuildId).")
```

If any steps called `report-incomplete`, enumerate them in a final
`report-incomplete` call:

```
call report-incomplete(
  reason="Smoke test detected <N> failing safe-output tool(s): <list>",
  context="See individual report-incomplete calls above for details."
)
```

---

## Operator Setup Checklist

Before registering this pipeline in Azure DevOps, complete the following:

- [ ] Create work item #1 → record ID as `smokeTestWorkItemId1` default
- [ ] Create work item #2 → record ID as `smokeTestWorkItemId2` default
- [ ] Create `smoke-test/fixture` branch and open PR → record PR ID as `smokeTestPullRequestId` default
- [ ] Add a comment to the PR → record thread ID as `smokeTestPrThreadId` default
- [ ] Identify or create a wiki → replace `PLACEHOLDER_WIKI_NAME` in front matter + record as `smokeTestWikiName` default
- [ ] Create `/smoke-test` root page in the wiki
- [ ] Identify a safe pipeline to trigger → add its ID to `queue-build.allowed-pipelines:` in front matter + record as `smokeTestQueueBuildPipelineId` default
- [ ] Recompile this file after editing front matter: `ado-aw compile examples/safe-outputs-smoke-test.md`
- [ ] Register the compiled pipeline in ADO and grant the Build Service identity the minimum permissions listed below

## Required ADO Permissions (Build Service identity)

Grant these to the **Project Collection Build Service** (or the identity
configured via `permissions.write`) in the ADO project:

| Scope | Permission | Reason |
|---|---|---|
| Work Items | Edit work items in this node | `create-work-item`, `comment-on-work-item`, `update-work-item`, `link-work-items`, `upload-workitem-attachment` |
| Repository (self) | Contribute, Create branch, Create tag | `create-pull-request`, `create-branch`, `create-git-tag` |
| Pull Requests (self) | Contribute to pull requests | `add-pr-comment`, `reply-to-pr-comment`, `resolve-pr-thread`, `submit-pr-review`, `update-pr` |
| Builds | Queue builds, Tag builds | `queue-build`, `add-build-tag` |
| Build Artifacts | `vso.build_execute` (granted by default via System.AccessToken) | `upload-pipeline-artifact`, `upload-build-attachment` |
| Wiki | Contribute | `create-wiki-page`, `update-wiki-page` |

See [docs/safe-output-permissions.md](../docs/safe-output-permissions.md) for
the ADO REST recipe to inspect and set these ACEs.
