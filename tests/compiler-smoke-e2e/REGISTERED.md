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
| `Candidate compiler smoke - janitor` | `ado-aw-mirror` | `tests/safe-outputs/janitor.lock.yml` | `2557` |
| `Candidate compiler smoke - failure reporter` | `ado-aw-mirror` | `tests/safe-outputs/smoke-failure-reporter.lock.yml` | `2558` |

All five child definitions use
`refs/heads/ado-aw-smoke-candidate-base` as their default branch. The ref is
permanent and inert; the harness never deletes it. Its seed commit is
`2b5fa7c336bd1f55a867cfc281e665472730b84c`.

## Security record

Before protected resources are authorized, record the verified fork settings
for the GitHub-backed orchestrator:

```text
forks.enabled=false
forks.allowSecrets=false
forks.allowFullAccessToken=false
pipelineTriggerSettings.buildsEnabledForForks=false
```

Existing GitHub-backed AgentPlayground definitions were explicitly hardened on
2026-07-22:

| Definition IDs | `forks.enabled` | `allowSecrets` | `allowFullAccessToken` | Effective fork builds |
| --- | --- | --- | --- | --- |
| `2544`–`2551` | `false` | `false` | `false` | `false` |
| `2559` | `false` | `false` | `false` | `false` |

Definition `2559` uses the `github.com_githubnext` GitHub service connection
and stores the five child definition IDs as non-secret definition variables.

Every child definition needs its own secret `GITHUB_TOKEN` for Copilot CLI
authentication. Definition `2558` additionally needs
`ADO_AW_DEBUG_GITHUB_TOKEN`; server-side definition cloning does not copy
secret values.

No secret values belong in this file.
