# Registered smoke pipelines

Definitions live in
[AgentPlayground](https://dev.azure.com/msazuresphere/AgentPlayground) under
`\smoke`.

## Lane definitions

These are **credential boundaries, not test cases**. Adding a smoke case does
not add a definition here; only a genuinely new credential class does.

| Definition | Repository | YAML path | Default branch | Definition ID |
| --- | --- | --- | --- | ---: |
| `ado-aw smoke lane - agentic` | `ado-aw-mirror` | `/.smoke/pipeline.yml` | `refs/heads/ado-aw-smoke-candidate-base` | _TBD_ |
| `ado-aw smoke lane - infra` | `ado-aw-mirror` | `/.smoke/pipeline.yml` | `refs/heads/ado-aw-smoke-candidate-base` | _not yet registered_ |

All are **API-queued only**: no CI trigger, no PR trigger, no schedule.
Their default branch is the permanent inert ref, so a lane cannot run without
an explicitly supplied case ref.

`infra` carries no cases yet, and a lane with no case in the running mode is
never resolved, so it needs no definition until the first `infra` case lands.

**Only `agentic` needs registering at cutover** — one definition for the whole
suite.

## Orchestrators

| Definition | Repository | YAML path | Triggers | Definition ID |
| --- | --- | --- | --- | ---: |
| `ado-aw candidate compiler smoke` | `githubnext/ado-aw` | `tests/smoke/azure-pipelines-candidate.yml` | PR (comment-gated) + nightly 01:00 UTC | `2559` |
| `ado-aw released smoke` | `githubnext/ado-aw` | `tests/smoke/azure-pipelines-release.yml` | scheduled daily 03:00 UTC | _TBD_ |

Both use the `github.com_githubnext` service connection.

## Supporting definitions

| Definition | Repository | YAML path | Purpose | Definition ID |
| --- | --- | --- | --- | ---: |
| `executor-e2e queue target` | `githubnext/ado-aw` | `tests/executor-e2e/queue-target.yml` | Queue target for the executor-e2e `queue-build` scenario (`E2E_QUEUE_PIPELINE_ID`) | _TBD_ |

## Orchestrator variables

Set on **both** orchestrator definitions — one per lane, never per case:

```text
SMOKE_LANE_AGENTIC_DEFINITION_ID
SMOKE_LANE_INFRA_DEFINITION_ID
```

Optional overrides:

```text
SMOKE_ARTIFACT_NAME=ado-aw-candidate
SMOKE_MIRROR_REPO=ado-aw-mirror
SMOKE_CONCURRENCY=5
SMOKE_CHILD_TIMEOUT_MS=7200000
SMOKE_POLL_MS=10000
SMOKE_STALE_REF_HOURS=24
```

## Secrets

ADO's server-side definition clone does **not** copy secret values; provision
them explicitly on each definition.

| Secret | On | Scope |
| --- | --- | --- |
| `GITHUB_TOKEN` | `agentic` lane | Copilot CLI authentication |
| `ADO_AW_GITHUB_TOKEN` | `agentic` lane | GitHub fine-grained PAT, Issues read/write limited to `jamesadevine/ado-aw-issues`. Read by the `create-github-issue` safe output in Stage 3 only — the compiler never projects it into Agent or Detection, and `assertAdoTokenIsolation` fails the run if it ever appears there. |

The `infra` lane holds no secrets, and nothing should ever be provisioned onto
it. Do not put either token in a variable group or on an orchestrator.

## Required permissions

The principal behind `agent-playground-write`, used only after artifact
publication, needs:

- Contribute / Create branch / Delete refs on `ado-aw-mirror`;
- Queue builds and Stop builds on the registered lane definitions;
- Read builds and artifacts in AgentPlayground.

Lane build identities need Code Read on `ado-aw-mirror` and, for candidate
mode, Build Read on the candidate orchestrator definition.

## One-time setup runbook

1. **Create the base ref.** On `ado-aw-mirror`, create
   `refs/heads/ado-aw-smoke-candidate-base` containing a single file
   `.smoke/pipeline.yml` with the contents of
   [`inert-child.yml`](inert-child.yml). If migrating, delete the five legacy
   placeholder lock paths in the same commit. The ref is permanent — the
   harness never deletes it.

2. **Register the `agentic` lane definition** against `ado-aw-mirror`, YAML path
   `/.smoke/pipeline.yml`, default branch as above. Create it explicitly
   (e.g. `az pipelines create --skip-run true`); `ado-aw enable` reuses an
   existing definition with the same YAML path, so it cannot create multiple
   definitions that share one.

   Only `agentic` is needed at cutover. `loadCases` resolves a definition id
   per lane *in play for the mode being run*, and every current case is
   `agentic`, so `infra` needs no definition and no variable until its first
   case lands. An unregistered lane cannot be queued by accident.

3. **Strip all triggers** on the lane: no CI, no PR, no schedule.

4. **Provision secrets** per the table above — `GITHUB_TOKEN` and
   `ADO_AW_GITHUB_TOKEN`, both on the `agentic` lane.

5. **Authorize service connections** (`agent-playground-read`,
   `agent-playground-write`) on the `agentic` lane.

6. **Register the released orchestrator** from
   `tests/smoke/azure-pipelines-release.yml` on `githubnext/ado-aw` via the
   `github.com_githubnext` connection, and harden its fork settings (below).

7. **Register the queue target** from `tests/executor-e2e/queue-target.yml`
   and set `E2E_QUEUE_PIPELINE_ID` on executor-e2e definition `2550` to its id.

8. **Set `SMOKE_LANE_AGENTIC_DEFINITION_ID`** on both orchestrators.

9. **Record every id** in the tables above and open a docs-only PR. In the same
   PR, repoint `scripts/rotate-agentplayground-secrets.ps1` at the lane:
   both `$copilotDefinitionIds` and `$reporterDefinitionIds` become the
   `agentic` lane id. Leaving the retired per-case ids there would rotate
   secrets onto definitions that no longer exist and silently skip the lane
   that actually runs.

10. **Trigger one manual run of each orchestrator** and check the live
    assertions in [`README.md`](README.md). ADO scheduled triggers do not fire
    until a definition has had at least one run.

11. **Only once both runs are green**, delete the retired definitions and
    remove `2545`–`2549` from
    [`trigger-policy.json`](trigger-policy.json) in the same commit. Deletion
    is not reversible, so this step is deliberately last.

## Security record

Every credentialed GitHub-backed definition that validates PRs must persist:

```text
forks.enabled=false
forks.allowSecrets=false
forks.allowFullAccessToken=false
pipelineTriggerSettings.buildsEnabledForForks=false
isCommentRequiredForPullRequest=true
isCommentRequiredForInternalRepoPRs=true
commentOptionInternalRepos=all
```

Hardened on 2026-07-22:

| Definition IDs | `forks.enabled` | `allowSecrets` | `allowFullAccessToken` | Effective fork builds |
| --- | --- | --- | --- | --- |
| `2544`, `2550` | `false` | `false` | `false` | `false` |
| `2559` | `false` | `false` | `false` | `false` |

Definition `2559` is optional on pull requests; a collaborator with write
access queues it with:

```text
/azp run ado-aw candidate compiler smoke
```

The released orchestrator has no PR trigger at all, so it is scheduled-only and
belongs in `scheduled_only_definition_ids` in
[`trigger-policy.json`](trigger-policy.json), alongside each registered lane.

No secret values belong in this file.

## Retired definitions

Superseded by the lane model, and deleted at cutover.

**Delete the ids from [`trigger-policy.json`](trigger-policy.json) first, in
the same commit that deletes the definitions.** `2545`–`2549` are currently in
`scheduled_only_definition_ids`, and the audit fetches every listed id with
`curl --fail-with-body`: a deleted definition returns 404, which fails
validation, exhausts all three retries, and aborts the run with *"Unable to
audit scheduled-only definition &lt;id&gt;"*. It fails closed rather than passing
silently, but it fails **every** smoke run until the file is corrected.

That is the only reason these ids are tracked. Once a definition is gone its
id means nothing: there is no rollback to re-enable and no trigger left to
drift. The table below is a record of what was removed, not a live registry.

| Definition IDs | Was | Replaced by |
| --- | --- | --- |
| `2545`–`2549` | Release-backed per-case smokes running committed `tests/safe-outputs/*.lock.yml` | Released-mode cases on the lane definitions |
| `2554`, `2555`, `2556`, `2558`, `2564`, `2565` | Candidate per-case smokes | Candidate-mode cases on the lane definitions |
| `2547` | Also served as the executor-e2e `queue-build` target | Dedicated `queue-target` definition |
| `2548` | Weekly janitor | `janitor` released-mode case (now daily; its 30-day prune window is idempotent) |
| `2557` | Candidate janitor | Retired earlier; not reinstated |

Only `2545`–`2549` appear in `trigger-policy.json`; the candidate-lane ids were
never listed. `2551` stays — it is trigger-e2e, not a retired smoke.

The deterministic E2E definitions are unaffected:

| Pipeline | Folder | Definition ID |
| --- | --- | ---: |
| ado-script e2e | `\ado-script-e2e` | `2544` |
| executor e2e | `\executor-e2e` | `2550` |
| trigger e2e | `\trigger-e2e` | `2551` |
| trigger e2e victim | `\trigger-e2e` | `2552` |
