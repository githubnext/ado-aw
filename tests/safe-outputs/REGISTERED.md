# Registered pipelines

Contributor-maintained mapping from smoke fixture → registered ADO
pipeline ID in
[AgentPlayground](https://dev.azure.com/msazuresphere/AgentPlayground).
After the manual-handoff registration step is complete, fill in the
`Pipeline ID` column and open a docs-only PR with the updates.

> ⚠️ `TBD` rows mean the fixture has been authored and committed but
> the corresponding ADO pipeline has not been registered yet. While any
> row is `TBD`, that safe output is **not** exercised in production.

| Fixture | Schedule | Pipeline ID | Notes |
| --- | --- | --- | --- |
| `noop.md` | `daily around 03:00` | `TBD` | Pilot smoke; no setup/teardown. |
| `missing-data.md` | `daily around 03:00` | `TBD` | NDJSON-only. |
| `missing-tool.md` | `daily around 03:00` | `TBD` | NDJSON-only. |
| `report-incomplete.md` | `daily around 03:00` | `TBD` | NDJSON-only. |
| `create-work-item.md` | `daily around 03:00` | `TBD` | Janitor prunes by prefix. |
| `comment-on-work-item.md` | `daily around 03:00` | `TBD` | References `$(permaWorkItemId)`. |
| `update-work-item.md` | `daily around 03:00` | `TBD` | References `$(permaWorkItemId)`. |
| `link-work-items.md` | `daily around 03:00` | `TBD` | References `$(permaWorkItemId)` and `$(permaWorkItem2Id)`. |
| `create-branch.md` | `daily around 03:00` | `TBD` | Janitor prunes by prefix. |
| `create-git-tag.md` | `daily around 03:00` | `TBD` | Janitor prunes by prefix. |
| `create-wiki-page.md` | `daily around 03:00` | `TBD` | References `$(permaWikiName)`. |
| `update-wiki-page.md` | `daily around 03:00` | `TBD` | References `$(permaWikiName)` and `$(permaWikiPagePath)`. |
| `add-build-tag.md` | `daily around 03:00` | `TBD` | Tags the current build; no cleanup needed. |
| `queue-build.md` | `daily around 03:00` | `TBD` | References `$(noopPipelineId)`. |
| `create-pull-request.md` | `daily around 03:00` | `TBD` | Janitor abandons transient PRs by prefix. |
| `add-pr-comment.md` | `daily around 03:00` | `TBD` | References `$(permaPullRequestId)`. |
| `reply-to-pr-comment.md` | `daily around 03:00` | `TBD` | References `$(permaPullRequestId)` and `$(permaThreadId)`. |
| `resolve-pr-thread.md` | `daily around 03:00` | `TBD` | Setup placeholder; needs real thread setup wired. |
| `submit-pr-review.md` | `daily around 03:00` | `TBD` | References `$(permaPullRequestId)`. |
| `update-pr.md` | `daily around 03:00` | `TBD` | References `$(permaPullRequestId)`; uses `update-description` operation only. |
| `upload-build-attachment.md` | `daily around 03:00` | `TBD` | Setup writes a small file under `$(Build.ArtifactStagingDirectory)`. |
| `upload-workitem-attachment.md` | `daily around 03:00` | `TBD` | Setup writes a small file; references `$(permaWorkItemId)`. |
| `upload-pipeline-artifact.md` | `daily around 03:00` | `TBD` | Setup writes a small file. |
| `noop-target.md` | _no schedule_ | `TBD` | Target of `queue-build.md`. Its pipeline ID populates `$(noopPipelineId)`. |
| `janitor.md` | `weekly on monday around 02:00` | `TBD` | Prunes `ado-aw-smoke-*` artifacts older than 30 days. |
| `smoke-failure-reporter.md` | `daily around 04:30` | `TBD` | Files `[smoke-failure] ...` issues on `githubnext/ado-aw`. Needs the `ADO_AW_DEBUG_GITHUB_TOKEN` secret pipeline variable, **only on this pipeline**. |

## Manual-handoff checklist

Before filling in the Pipeline IDs above, the operator must complete
the following one-time setup in
`https://dev.azure.com/msazuresphere/AgentPlayground`:

1. Confirm or create service connections `agent-playground-read` and
   `agent-playground-write` (used by the compiled pipelines at runtime
   via the front-matter `permissions:` block; not to be confused with
   the GitHub service connection in step 4).
2. Create branch `daily-smoke-target` and open a perma-PR
   `daily-smoke perma-PR (do not merge)` from `daily-smoke-target` →
   `main` with one comment thread for `reply-to-pr-comment` /
   `resolve-pr-thread`.
3. Create variable group `ado-aw-daily-smoke` containing:
   - `permaWorkItemId`, `permaWorkItem2Id` (two long-lived work items).
   - `permaPullRequestId`, `permaThreadId` (from step 2).
   - `permaWikiName`, `permaWikiPagePath` (a wiki + a long-lived page).
   - `noopPipelineId` (filled in after step 5).
4. **Create the GitHub service connection.** Project settings →
   Service connections → **New service connection → GitHub**. Either
   install the Azure Pipelines GitHub App on `githubnext/ado-aw` (no
   long-lived secret) or paste a fine-grained PAT scoped to the repo.
   Name it something memorable (e.g. `ado-aw-github`) — that name is
   passed to `--service-connection` in step 5.
5. **Bulk-register the smoke pipelines with `ado-aw enable`.** From a
   `githubnext/ado-aw` checkout:
   ```powershell
   ado-aw enable `
     --org msazuresphere --project AgentPlayground `
     --service-connection ado-aw-github `
     --folder '\smoke' `
     tests/safe-outputs/
   ```
   `enable` autodetects the GitHub remote and emits the GitHub-shaped
   create-definition body for every `*.lock.yml` under
   `tests/safe-outputs/`. Re-running is idempotent.
6. Register the `noop-target.lock.yml` pipeline (covered by step 5)
   and update `noopPipelineId` in the variable group with the
   captured ID.
7. Capture each new Pipeline ID from the `enable` output (or via
   `ado-aw list --org msazuresphere --project AgentPlayground`) and
   update the column above; open a docs-only PR.
8. Provision pipeline variable `ADO_AW_DEBUG_GITHUB_TOKEN` (secret) on
   the `smoke-failure-reporter` pipeline **only**. Use a GitHub
   fine-grained PAT scoped to `Issues: Read and write` on
   `githubnext/ado-aw` only. Do **not** put this token in the variable
   group — it must not be reachable by the smoke pipelines themselves.
   ```powershell
   ado-aw secrets set ADO_AW_DEBUG_GITHUB_TOKEN `
     --org msazuresphere --project AgentPlayground `
     --definition-ids <smoke-failure-reporter-pipeline-id> `
     --value <fine-grained-pat>
   ```
9. **Trigger one manual run per pipeline.** ADO's scheduled triggers
   do not fire until each definition has had at least one successful
   run. From the same checkout:
   ```powershell
   ado-aw run --org msazuresphere --project AgentPlayground tests/safe-outputs/
   ```
   After that, the daily schedule baked into each smoke's front-matter
   takes over.
