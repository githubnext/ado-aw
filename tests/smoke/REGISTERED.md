# Registered smoke pipelines

Definitions live in the
[`outlookweb/Test`](https://dev.azure.com/outlookweb/Test) project under
`\smoke`. They moved there from `msazuresphere/AgentPlayground`; see
[*Migration record*](#migration-record) at the end of this file.

## Lane definitions

These are **credential boundaries, not test cases**. Adding a smoke case does
not add a definition here; only a genuinely new credential class does.

| Definition | Repository | YAML path | Default branch | Definition ID |
| --- | --- | --- | --- | ---: |
| `ado-aw smoke lane - agentic` | `ado-aw-mirror` | `/.smoke/pipeline.yml` | `refs/heads/ado-aw-smoke-candidate-base` | `2875` |
| `ado-aw smoke lane - infra` | `ado-aw-mirror` | `/.smoke/pipeline.yml` | `refs/heads/ado-aw-smoke-candidate-base` | _not yet registered_ |

All are **API-queued only**: no CI trigger, no PR trigger, no schedule.
Their default branch is the permanent inert ref, so a lane cannot run without
an explicitly supplied case ref.

`infra` carries no cases yet, and a lane with no case in the running mode is
never resolved, so it needs no definition until the first `infra` case lands.

## Orchestrators

The orchestrators are **not** replaced by the lane model, and are not
themselves smoke cases. A lane runs a staged `.smoke/pipeline.yml` from
`ado-aw-mirror`; an orchestrator runs from `githubnext/ado-aw`, builds or
downloads the compiler, publishes the candidate artifact, stages each case to
its own ref, and queues the lane. That work cannot live in a lane — it is what
*drives* the lanes.

| Definition | Repository | YAML path | Triggers | Definition ID |
| --- | --- | --- | --- | ---: |
| `ado-aw candidate compiler smoke` | `githubnext/ado-aw` | `tests/smoke/azure-pipelines-candidate.yml` | PR (comment-gated) + nightly 01:00 UTC | _pending_ |
| `ado-aw released smoke` | `githubnext/ado-aw` | `tests/smoke/azure-pipelines-release.yml` | scheduled daily 03:00 UTC | _pending_ |

Both use the project's GitHub service connection to `githubnext`.

## Supporting definitions

| Definition | Repository | YAML path | Purpose | Definition ID |
| --- | --- | --- | --- | ---: |
| `executor-e2e queue target` | `githubnext/ado-aw` | `tests/executor-e2e/queue-target.yml` | Queue target for the executor-e2e `queue-build` scenario (`E2E_QUEUE_PIPELINE_ID`) | _pending_ |

The deterministic E2E definitions:

| Pipeline | Folder | Repository | Definition ID |
| --- | --- | --- | ---: |
| ado-script e2e | `\ado-script-e2e` | `githubnext/ado-aw` | _pending_ |
| executor e2e | `\executor-e2e` | `githubnext/ado-aw` | _pending_ |
| trigger e2e | `\trigger-e2e` | `githubnext/ado-aw` | _pending_ |
| trigger e2e victim | `\trigger-e2e` | `ado-aw-mirror` | `2876` |

## Agent pool

Every pipeline runs on the Microsoft-hosted **Azure Pipelines** queue (`930`)
with `vmImage: ubuntu-22.04` — the same image the compiler defaults to.
`outlookweb` auto-injects *Component Detection* and *Secure Supply Chain
Analysis* tasks into every job by org policy; they are expected in timelines.

Hosted parallel-job capacity is shared with the rest of the org. If runs queue
for long periods, lower `SMOKE_CONCURRENCY` / `TRIGGER_E2E_CONCURRENCY` on the
orchestrator definitions.

## Orchestrator variables

Set on **both** orchestrator definitions — one per lane, never per case:

```text
SMOKE_LANE_AGENTIC_DEFINITION_ID=2875
SMOKE_LANE_INFRA_DEFINITION_ID      (unset until the infra lane exists)
```

Optional overrides:

```text
SMOKE_ARTIFACT_NAME=ado-aw-candidate
SMOKE_MIRROR_REPO=ado-aw-mirror
SMOKE_CONCURRENCY=5
SMOKE_CHILD_TIMEOUT_MS=7200000
SMOKE_POLL_MS=10000
SMOKE_STALE_REF_HOURS=24
CRATES_IO_FEED=sparse+https://pkgs.dev.azure.com/outlookweb/Test/_packaging/ado-aw-ci-rust/Cargo/index/
```

## Secrets

ADO does not copy secret values between definitions, and never across
organizations. Provision them explicitly on each definition with
[`scripts/rotate-ci-secrets.ps1`](../../scripts/rotate-ci-secrets.ps1).

| Secret | On | Scope |
| --- | --- | --- |
| `GITHUB_TOKEN` | `agentic` lane (`2875`) | Copilot CLI authentication |
| `EXECUTOR_E2E_GITHUB_TOKEN` | executor e2e | Fine-grained PAT, Issues read/write on the failure-report repo |
| `TRIGGER_E2E_GITHUB_TOKEN` | trigger e2e | Fine-grained PAT, Issues read/write on the failure-report repo |
| `ADO_AW_GITHUB_TOKEN` | *(none)* | GitHub PAT for `create-github-issue`. No case files GitHub issues today; provision it on the `agentic` lane if one adopts that safe output. |

The `infra` lane holds no secrets, and nothing should ever be provisioned onto
it. Do not put any of these tokens in a variable group or on an orchestrator.

## Project resources

| Resource | Purpose |
| --- | --- |
| Repo `ado-aw-mirror` | Smoke staging repo and trigger-e2e victim `self`. Holds `main` (fast-forwarded from GitHub by trigger-e2e) and the permanent inert `ado-aw-smoke-candidate-base`. |
| Repo `ado-aw-e2e-fixture` | Immutable `e2e/*` refs for ado-script e2e; `main` checked out by the `multi-repo` case. |
| Repo `agent-definitions` | Executor-e2e scratch repository. |
| Wiki `Test.wiki` | Executor-e2e wiki scenarios (auto-discovered). |
| Feed `ado-aw-ci-rust` | Rust toolchain (`RustInstaller@1`, NuGet upstream `devdiv-DevDiv-Rust.Sdk@Release`) and crates.io mirror (Cargo upstream `https://index.crates.io/`). |
| Feed `ado-aw-ci-test-feed` | `permissions.packages` smoke target (npmjs, NuGet Gallery, PyPI upstreams). |
| Service connection `ado-aw-ci-read` | Workload-identity Azure Resource Manager connection; read-only ADO token for Stage 1. |
| Service connection `ado-aw-ci-write` | Workload-identity Azure Resource Manager connection; write-capable ADO token for Stage 3, ref staging, and queueing. |
| GitHub service connection | `githubnext` via the Azure Pipelines GitHub App. |

## Required permissions

The principal behind `ado-aw-ci-write`, used only after artifact publication,
needs:

- Contribute / Create branch / Delete refs / Contribute to pull requests on
  `ado-aw-mirror` and `agent-definitions`;
- Queue builds on the registered lane, victim, and queue-target definitions;
- project-level **Stop builds** (gate self-cancel is checked against a
  build-instance token, not the definition);
- Read builds and artifacts, and edit build quality / tags, in the project;
- work-item write in the project (canary, executor e2e).

Lane and victim build identities (`Test Build Service (outlookweb)`, because
the project enforces job authorization scope) need Code Read on
`ado-aw-mirror` and, for candidate mode, Build Read on the candidate
orchestrator definition.

Both feeds grant `Test Build Service (outlookweb)` **Feed and Upstream Reader
(Collaborator)**; `ado-aw-ci-test-feed` grants the same role to the
`ado-aw-ci-read` service principal. Collaborator, not Reader: both feeds rely
on upstreams, and only Collaborator can save an upstream package on first
read.

A new lane definition also needs the **agent pool** explicitly authorized for
it, and a **repository resource** authorized for every extra repo any of its
cases checks out. Both are distinct grants from the service connections, and
both stall builds silently rather than failing them: the build queues, sits
at `status: notStarted` indefinitely, and its timeline shows
`Checkpoint.Authorization: inProgress`.

```
PATCH _apis/pipelines/pipelinePermissions/queue/930?api-version=7.1-preview.1
PATCH _apis/pipelines/pipelinePermissions/repository/<projectId>.<repoId>?api-version=7.1-preview.1
{ "pipelines": [ { "id": <definitionId>, "authorized": true } ] }
```

**Any case adding a new `repos:` entry needs the repository grant for the
lane**, once, before that case can run. It is the one piece of per-case ADO
setup the lane model does not remove — check it first whenever a case hangs
with no logs.

## Setup runbook (`outlookweb/Test`)

1. ✅ **Repos created and seeded.** `ado-aw-e2e-fixture` and
   `agent-definitions` were mirrored from AgentPlayground with all branches and
   tags (scratch `ado-aw-det-*` branches pruned). `ado-aw-mirror` was seeded
   with `main` at the GitHub `main` commit and
   `refs/heads/ado-aw-smoke-candidate-base` = `main` +
   `.smoke/pipeline.yml` containing [`inert-child.yml`](inert-child.yml).
2. ✅ **Wiki created** (`Test.wiki`).
3. ✅ **`agentic` lane registered as `2875`** against `ado-aw-mirror`, YAML
   path `/.smoke/pipeline.yml`, default branch as above, `triggers: null`.
   Queue `930` and repository resource `ado-aw-e2e-fixture` authorized.
4. ✅ **Lane wiring verified live.** Build `5012660` on the base ref failed at
   `Reject inert candidate-smoke base`. That failure is the **pass condition**
   — it proves checkout, pool, YAML path and the inert guard all work. Re-run
   it after any lane change.
5. ✅ **Trigger-e2e victim registered as `2876`** against `ado-aw-mirror`
   (`tests/trigger-e2e/victim-pipeline.yml`, default branch `main`), queue
   `930` authorized.
6. ⏳ **Feeds** — needs an `outlookweb` feed creator. Create `ado-aw-ci-rust`
   and `ado-aw-ci-test-feed` as described under *Project resources* and grant
   the roles under *Required permissions*. `ado-aw-ci-test-feed-not-granted`
   must **not** exist: the ungranted-feed check must be refused by policy,
   never by a real feed.
7. ⏳ **Service connections** — needs an Entra app owner and a project admin.
   Create `ado-aw-ci-read` / `ado-aw-ci-write` (workload identity federation),
   add each service principal to `outlookweb` with access to `Test`, grant
   the permissions above, and authorize both connections on every definition
   that names them (lane, victim, all three orchestrators, executor e2e). Then
   create the GitHub connection to `githubnext` through the Azure Pipelines
   GitHub App (interactive).
8. ⏳ **GitHub-backed definitions** — register from `githubnext/ado-aw`:
   queue target, executor e2e, trigger e2e, ado-script e2e, candidate and
   released orchestrators. For each: authorize queue `930`, the service
   connections it names, and `ado-aw-e2e-fixture` for ado-script e2e. Apply
   the *Security record* settings to every PR-triggered one.
9. ⏳ **Variables** — `SMOKE_LANE_AGENTIC_DEFINITION_ID=2875` on both
   orchestrators; `E2E_QUEUE_PIPELINE_ID`, `EXECUTOR_E2E_ISSUE_REPO`,
   `E2E_WORK_ITEM_ASSIGNEE` on executor e2e;
   `TRIGGER_E2E_VICTIM_DEFINITION_ID=2876`, `TRIGGER_E2E_VICTIM_REPO=ado-aw-mirror`,
   `TRIGGER_E2E_ISSUE_REPO`, `TRIGGER_E2E_BUILD_TIMEOUT_MS=1800000` on trigger
   e2e.
10. ⏳ **IDs recorded** — fill every _pending_ ID in this file,
    [`trigger-policy.json`](trigger-policy.json) (an empty
    `pr_definition_ids` fails the audit closed), and
    [`rotate-ci-secrets.ps1`](../../scripts/rotate-ci-secrets.ps1).
11. ⏳ **Secrets** — run `rotate-ci-secrets.ps1`.
12. ⏳ **Live green runs** of the candidate orchestrator, released
    orchestrator, executor e2e, trigger e2e and ado-script e2e. Trigger e2e
    syncs `ado-aw-mirror` `main` fast-forward-only from GitHub `main`, so its
    first green run comes after this change merges.
13. ⏳ **Cutover** — disable the AgentPlayground definitions (see
    *Migration record*).

## Package-feed cases

`package-feeds` ([`package-feeds.md`](package-feeds.md)) and
`package-feeds-wif` ([`package-feeds-wif.md`](package-feeds-wif.md)) prove
`permissions.packages` end to end. npm, pip, and dotnet restore from an Azure
Artifacts feed through `ado-proxy` with no feed credential in the sandbox.
The first case uses the build identity, the second a workload-identity token.
The build-identity case also proves that ungranted feeds are refused and that
`public-registry: block` removes the public registry.

Both run on the `agentic` lane; no new definition or service-connection
authorization is needed. **Removing `ado-aw-ci-test-feed`, or either of its
Collaborator grants, turns both cases red.** If a preflight fails, the
`Start ado-proxy policy engine` step names the feed, the identity, and the
role to grant.

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

The project-level setting `isCommentRequiredForPullRequest` is **off** in
`outlookweb/Test`, so the comment gate must be set on each definition.

The candidate orchestrator is optional on pull requests; a collaborator with
write access queues it with:

```text
/azp run ado-aw candidate compiler smoke
```

The released orchestrator has no PR trigger at all, so it is scheduled-only and
belongs in `scheduled_only_definition_ids` in
[`trigger-policy.json`](trigger-policy.json), alongside each registered lane,
the trigger-e2e definitions, and the queue target.

No secret values belong in this file.

## Migration record

CI moved from `msazuresphere/AgentPlayground` to `outlookweb/Test`. The old
definitions are disabled at cutover and are not referenced anywhere in this
repository:

| AgentPlayground ID | Was |
| --- | --- |
| `2544` | ado-script e2e |
| `2550` | executor e2e |
| `2551` / `2552` | trigger e2e orchestrator / victim |
| `2559` | candidate compiler smoke |
| `2567` | `agentic` smoke lane |
| `2568` | released smoke |
| `2569` | executor-e2e queue target |
| `2545`–`2549`, `2554`–`2565` | retired per-case smokes from before the lane model; not migrated, deleted at cutover |
