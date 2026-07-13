# Registered pipelines

Contributor-maintained mapping from smoke fixture → registered ADO
pipeline ID in
[AgentPlayground](https://dev.azure.com/msazuresphere/AgentPlayground).
After the manual-handoff registration step is complete, fill in the
`Pipeline ID` column and open a docs-only PR with the updates.

> ⚠️ `TBD` rows mean the fixture has been authored and committed but
> the corresponding ADO pipeline has not been registered yet. While any
> row is `TBD`, that pipeline is **not** exercised in production.

| Fixture | Schedule | Pipeline ID | Notes |
| --- | --- | --- | --- |
| `canary.md` | `daily around 03:00` | `TBD` | Omnibus: noop + create-work-item + add-build-tag in one agentic run. Proves Stage 1 → 2 → 3 end-to-end. |
| `azure-cli.md` | `daily around 03:00` | `TBD` | Verifies AWF az CLI mount + ADO auth via `AZURE_DEVOPS_EXT_PAT`. |
| `noop-target.md` | _no schedule_ | `TBD` | Target of the `queue-build` executor-e2e scenario. Its Pipeline ID must be set as `E2E_QUEUE_PIPELINE_ID` on the executor-e2e pipeline. |
| `janitor.md` | `weekly on monday around 02:00` | `TBD` | Prunes `ado-aw-smoke-*` artifacts older than 30 days. |
| `smoke-failure-reporter.md` | `daily around 04:30` | `TBD` | Files `[smoke-failure] …` issues on `githubnext/ado-aw`. Requires the `ADO_AW_DEBUG_GITHUB_TOKEN` secret pipeline variable, **only on this pipeline**. |

## Manual-handoff checklist

Before filling in the Pipeline IDs above, the operator must complete
the following one-time setup in
`https://dev.azure.com/msazuresphere/AgentPlayground`:

1. Confirm or create service connections `agent-playground-read` and
   `agent-playground-write`.
2. **Bulk-register the smoke pipelines with `ado-aw enable`.** From a
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
3. Capture each new Pipeline ID from the `enable` output (or via
   `ado-aw list`) and update the table above; open a docs-only PR.
4. **Set `E2E_QUEUE_PIPELINE_ID`** on the executor-e2e pipeline
   (`tests/executor-e2e/azure-pipelines.yml`) with the `noop-target`
   Pipeline ID from step 3. This enables the `queue-build` deterministic
   scenario.
5. Provision pipeline variable `ADO_AW_DEBUG_GITHUB_TOKEN` (secret) on
   the `smoke-failure-reporter` pipeline **only**. Use a GitHub
   fine-grained PAT scoped to `Issues: Read and write` on
   `githubnext/ado-aw` only.

   ```powershell
   ado-aw secrets set ADO_AW_DEBUG_GITHUB_TOKEN `
     --org msazuresphere --project AgentPlayground `
     --definition-ids <smoke-failure-reporter-pipeline-id> `
     --value <fine-grained-pat>
   ```

6. **Trigger one manual run per pipeline.** ADO's scheduled triggers
   do not fire until each definition has had at least one successful
   run:

   ```powershell
   ado-aw run --org msazuresphere --project AgentPlayground tests/safe-outputs/
   ```
