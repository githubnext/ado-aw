# Registered smoke pipelines

Definitions live in
[AgentPlayground](https://dev.azure.com/msazuresphere/AgentPlayground) under
`\smoke`.

## Lane definitions

These are **credential boundaries, not test cases**. Adding a smoke case does
not add a definition here; only a genuinely new credential class does.

| Definition | Repository | YAML path | Default branch | Definition ID |
| --- | --- | --- | --- | ---: |
| `ado-aw smoke lane - agentic` | `ado-aw-mirror` | `/.smoke/pipeline.yml` | `refs/heads/ado-aw-smoke-candidate-base` | `2567` |
| `ado-aw smoke lane - infra` | `ado-aw-mirror` | `/.smoke/pipeline.yml` | `refs/heads/ado-aw-smoke-candidate-base` | _not yet registered_ |

All are **API-queued only**: no CI trigger, no PR trigger, no schedule.
Their default branch is the permanent inert ref, so a lane cannot run without
an explicitly supplied case ref.

`infra` carries no cases yet, and a lane with no case in the running mode is
never resolved, so it needs no definition until the first `infra` case lands.

**Only `agentic` needs registering at cutover** — one definition for the whole
suite.

## Orchestrators

The orchestrators are **not** replaced by the lane model, and are not
themselves smoke cases. A lane runs a staged `.smoke/pipeline.yml` from
`ado-aw-mirror`; an orchestrator runs from `githubnext/ado-aw`, builds or
downloads the compiler, publishes the candidate artifact, stages each case to
its own ref, and queues the lane. That work cannot live in a lane — it is what
*drives* the lanes.

What the lane model replaced is the **per-case child definitions**
(`2554`–`2565`), one per test case. Those are in the retirement table below.

| Definition | Repository | YAML path | Triggers | Definition ID |
| --- | --- | --- | --- | ---: |
| `ado-aw candidate compiler smoke` | `githubnext/ado-aw` | `tests/smoke/azure-pipelines-candidate.yml` | PR (comment-gated) + nightly 01:00 UTC | `2559` |
| `ado-aw released smoke` | `githubnext/ado-aw` | `tests/smoke/azure-pipelines-release.yml` | scheduled daily 03:00 UTC | _TBD_ |

Both use the `github.com_githubnext` service connection.

> **Definition `2559` still points at the OLD path**
> (`/tests/compiler-smoke-e2e/azure-pipelines.yml`), which this change deletes.
> Its `process.yamlFilename` **must** be repointed at
> `/tests/smoke/azure-pipelines-candidate.yml` when the PR merges, or the
> candidate orchestrator breaks on its next run. It cannot be repointed in
> advance, because the new path does not exist on `main` until then.

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
| `ADO_AW_GITHUB_TOKEN` | *(none currently)* | GitHub fine-grained PAT for `create-github-issue`. No case files GitHub issues today; provision it on the `agentic` lane if one adopts that safe output. |

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

A new lane definition also needs the **agent pool** explicitly authorized for
it (see step 5b) — this is a distinct grant from the service connections, and
its absence stalls builds silently rather than failing them.

## One-time setup runbook

Steps 1–3, 5 and part of 8 are **already done** (see the ✅ marks). The rest
either need a credential no checkout has, or a file that only exists once this
PR merges.

1. ✅ **Base ref created.** `refs/heads/ado-aw-smoke-candidate-base` on
   `ado-aw-mirror` now carries `.smoke/pipeline.yml` with the contents of
   [`inert-child.yml`](inert-child.yml) (commit `1d173bc`). The ref is
   permanent — the harness never deletes it.

   The legacy `tests/**/*.lock.yml` paths on that ref were **deliberately left
   in place**: the ten retired definitions still point at them, so deleting
   them before cutover would break the currently-running smokes. They go with
   the definitions in step 11.

2. ✅ **`agentic` lane registered as `2567`** against `ado-aw-mirror`, YAML
   path `/.smoke/pipeline.yml`, default branch as above.

   Only `agentic` is needed. `loadCases` resolves a definition id per lane *in
   play for the mode being run*, and every current case is `agentic`, so
   `infra` needs no definition and no variable until its first case lands. An
   unregistered lane cannot be queued by accident.

3. ✅ **No triggers on `2567`** — verified `triggers: null`, so it is
   API-queued only.

4. ✅ **`GITHUB_TOKEN` provisioned on `2567`.**
   `scripts/rotate-agentplayground-secrets.ps1` covers `2567`.

   `ADO_AW_GITHUB_TOKEN` is also present but currently **unused** — no case
   files GitHub issues since `smoke-failure-reporter` was removed. Harmless
   (nothing reads it), and it can be deleted until [#1796](https://github.com/githubnext/ado-aw/issues/1796)
   lands, which puts it on the *orchestrator* rather than the lane.

5. ✅ **Service connections authorized** on `2567`:
   `agent-playground-read` and `agent-playground-write`.

5b. ✅ **Agent pool authorized** on `2567` (queue `1453`,
   `AZS-1ES-L-Playground-ubuntu-22.04`).

   Easy to miss, and it does **not** surface as an error: the build queues,
   sits at `status: notStarted` indefinitely, and its timeline shows
   `Checkpoint.Authorization: inProgress`. There is no failure and no
   timeout — it simply never starts. Pool authorization is separate from
   service-connection authorization:

   ```
   PATCH _apis/pipelines/pipelinePermissions/queue/1453?api-version=7.1-preview.1
   { "pipelines": [ { "id": <definitionId>, "authorized": true } ] }
   ```

5c. ✅ **Lane wiring verified live.** Queued `2567` on the base ref
   (build `629504`); it failed at `Reject inert candidate-smoke base` with
   *"Candidate compiler smoke must be queued with an explicit generated ref."*

   That failure is the **pass condition** — it proves checkout, pool, YAML
   path and the inert guard all work, and that a lane cannot run without an
   explicitly supplied case ref. Re-run this after any lane change.

6. ⏳ **Register the released orchestrator** from
   `tests/smoke/azure-pipelines-release.yml` via the `github.com_githubnext`
   connection, and harden its fork settings (below). *Blocked until merge —
   the file does not exist on `main` yet.*

7. ⏳ **Register the queue target** from `tests/executor-e2e/queue-target.yml`,
   then set `E2E_QUEUE_PIPELINE_ID` on executor-e2e definition `2550` to its
   id. It is currently `2547`, which step 11 deletes. *Blocked until merge.*

8. **Set `SMOKE_LANE_AGENTIC_DEFINITION_ID`** on both orchestrators.
   ✅ Done on `2559`; the released orchestrator gets it at step 6.

9. ⏳ **Repoint `2559`** at `/tests/smoke/azure-pipelines-candidate.yml` — see
   the warning above — and in the same edit delete its six now-dead
   `COMPILER_SMOKE_*_DEFINITION_ID` variables. They must go *together*: the old
   orchestrator YAML reads those variables, and the new one never does, so
   removing them earlier breaks the running smoke and leaving them afterwards
   preserves pointers to deleted definitions. *Do this at merge, before the
   next scheduled run.*

10. ⏳ **Trigger one manual run of each orchestrator** and check the live
    assertions in [`README.md`](README.md). ADO scheduled triggers do not fire
    until a definition has had at least one run.

11. ⏳ **Only once both runs are green**, delete the retired definitions, drop
    the legacy lock paths from the base ref, and remove `2545`–`2549` from
    [`trigger-policy.json`](trigger-policy.json) in the same commit. Deletion
    is not reversible, so this step is deliberately last.

12. ⏳ **Repoint `scripts/rotate-agentplayground-secrets.ps1`** at the lane:
    both `$copilotDefinitionIds` and `$reporterDefinitionIds` become `2567`.
    Leaving the retired per-case ids there would rotate secrets onto
    definitions that no longer exist and silently skip the lane that runs.

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
