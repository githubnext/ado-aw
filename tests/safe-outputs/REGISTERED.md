# Registered pipelines

Contributor-maintained mapping from smoke fixture → registered ADO
pipeline ID in
[AgentPlayground](https://dev.azure.com/msazuresphere/AgentPlayground).
The table records the active definitions after the July 2026 replacement-first
cutover.

| Fixture | Schedule | Pipeline ID | Notes |
| --- | --- | --- | --- |
| `canary.md` | `daily around 03:00` | `2545` | Omnibus: noop + create-work-item + add-build-tag in one agentic run. Proves Stage 1 → 2 → 3 end-to-end. |
| `azure-cli.md` | `daily around 03:00` | `2546` | Verifies AWF az CLI mount + ADO auth via `AZURE_DEVOPS_EXT_PAT`. |
| `noop-target.md` | _no schedule_ | `2547` | Target of the `queue-build` executor-e2e scenario. `E2E_QUEUE_PIPELINE_ID=2547` on executor definition `2550`. |
| `janitor.md` | `weekly on monday around 02:00` | `2548` | Prunes `ado-aw-smoke-*` artifacts older than 30 days. |
| `smoke-failure-reporter.md` | `daily around 04:30` | `2549` | Files `[smoke-failure] …` issues on `jamesadevine/ado-aw-issues`. Requires the `ADO_AW_DEBUG_GITHUB_TOKEN` secret pipeline variable, **only on this pipeline**. |

## Deterministic E2E definitions

| Pipeline | Folder | Pipeline ID | Notes |
| --- | --- | ---: | --- |
| ado-script e2e | `\ado-script-e2e` | `2544` | Azure-native checkout regression suite. |
| executor e2e | `\executor-e2e` | `2550` | Deterministic Stage 3 coverage for every non-debug safe output. |
| trigger e2e | `\trigger-e2e` | `2551` | Concurrent gate/synthetic-PR orchestrator. |
| trigger e2e victim | `\trigger-e2e` | `2552` | Azure Repos-backed victim using `ado-aw-mirror`. |

All GitHub-backed definitions use the `githubnext` service connection. The
mirror and every AgentPlayground test repository carry root
`es-metadata.yml` inventory metadata so repository inventory automation keeps
them enabled.

The PR/nightly compiler-candidate definitions are tracked separately in
[`tests/compiler-smoke-e2e/REGISTERED.md`](../compiler-smoke-e2e/REGISTERED.md);
they do not replace or repoint the release-backed definitions above.

## Retired definitions

The cutover deleted legacy definitions `2506`, `2513` through `2538`, and
`2541`. Per-tool agentic coverage is now split between canary `2545` (full
Agent → Detection → SafeOutputs handoff) and executor E2E `2550`
(deterministic per-tool execution and effect assertions).

## Manual-handoff checklist

Before filling in the Pipeline IDs above, the operator must complete
the following one-time setup in
`https://dev.azure.com/msazuresphere/AgentPlayground`:

1. Confirm or create service connections `agent-playground-read` and
   `agent-playground-write`.
2. **Bulk-register the smoke pipelines with `ado-aw enable`.** From a
   `githubnext/ado-aw` checkout:

   ```powershell
   cargo run -- enable `
     --org msazuresphere --project AgentPlayground `
     --service-connection githubnext `
     --also-set-token `
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
   `jamesadevine/ado-aw-issues` only. Confirm the target repository has the
   `pipeline-failure` and `ado-aw-smoke` labels.

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

For the AgentPlayground replacement-first migration, create side-by-side
definitions explicitly with `az pipelines create --skip-run true`.
`ado-aw enable` intentionally reuses an existing definition with the same YAML
path, so it cannot create replacements while the legacy definitions remain.
