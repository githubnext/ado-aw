# Registered candidate compiler smoke pipelines

These definitions live in
[AgentPlayground](https://dev.azure.com/msazuresphere/AgentPlayground) under
`\compiler-smoke-e2e`.

| Pipeline | Repository | YAML path | Definition ID |
| --- | --- | --- | ---: |
| `ado-aw candidate compiler smoke` | `githubnext/ado-aw` | `tests/compiler-smoke-e2e/azure-pipelines.yml` | `2559` |
| `Candidate compiler smoke - canary` | `ado-aw-mirror` | `tests/safe-outputs/canary.lock.yml` | `2554` |
| `Candidate compiler smoke - azure-cli` | `ado-aw-mirror` | `tests/safe-outputs/azure-cli.lock.yml` | `2555` |
| `Candidate compiler smoke - noop-target` | `ado-aw-mirror` | `tests/safe-outputs/noop-target.lock.yml` | `2556` |
| `Candidate compiler smoke - failure reporter` | `ado-aw-mirror` | `tests/safe-outputs/smoke-failure-reporter.lock.yml` | `2558` |
| `Candidate compiler smoke - custom safe outputs` | `ado-aw-mirror` | `tests/compiler-smoke-e2e/custom-safe-output.lock.yml` | `2564` |

All five child definitions use
`refs/heads/ado-aw-smoke-candidate-base` as their default branch. The ref is
permanent and inert; the harness never deletes it.

Candidate janitor definition `2557` was retired. The release-backed janitor
definition `2548` remains scheduled weekly and is not part of this lane.

The custom child imports
`AgentPlayground/ado-aw-e2e-fixture/components/custom-build-tags/component.md`
from immutable ref `refs/heads/e2e/custom-safe-output-v1`, pinned to
`aa711dd17c4dfcde492b2bfad62e5fb1baad71f6`. Definition `2564` is explicitly
authorized for that repository resource.

## Security record

Before protected resources are authorized, record the verified fork settings
for the GitHub-backed orchestrator:

```text
forks.enabled=false
forks.allowSecrets=false
forks.allowFullAccessToken=false
pipelineTriggerSettings.buildsEnabledForForks=false
isCommentRequiredForPullRequest=true
isCommentRequiredForInternalRepoPRs=true
commentOptionInternalRepos=all
```

GitHub-backed AgentPlayground definitions that intentionally validate PRs were
explicitly hardened on 2026-07-22:

| Definition IDs | `forks.enabled` | `allowSecrets` | `allowFullAccessToken` | Effective fork builds |
| --- | --- | --- | --- | --- |
| `2544`, `2550` | `false` | `false` | `false` | `false` |
| `2559` | `false` | `false` | `false` | `false` |

Definition `2559` is optional on pull requests. A collaborator with repository
write access queues it from the PR with:

```text
/azp run ado-aw candidate compiler smoke
```

Its nightly `main` schedule remains independent and runs at 01:00 UTC with
`always: true`.

Release-smoke definitions `2545`-`2549` and scheduled trigger E2E definition
`2551` have no CI or PR trigger metadata. Their schedules/manual queues remain
independent of the candidate compiler PR lane.

Definition `2559` uses the `github.com_githubnext` GitHub service connection
and stores the five child definition IDs as non-secret definition variables.

Every child definition needs its own secret `GITHUB_TOKEN` for Copilot CLI
authentication. Definition `2558` additionally needs
`ADO_AW_DEBUG_GITHUB_TOKEN`; server-side definition cloning does not copy
secret values.

The custom child ID is configured on `2559` as
`COMPILER_SMOKE_CUSTOM_SAFE_OUTPUT_DEFINITION_ID=2564`.

No secret values belong in this file.
