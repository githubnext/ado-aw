# Azure-native ado-script E2E

This pipeline validates `prepare-pr-base.js` against Azure Pipelines' real
checkout behavior. GitHub Actions covers TypeScript and local Git logic, but it
cannot reproduce Azure Repos resource checkout, predefined variables, or
`$(System.AccessToken)` authentication.

The fixture repository is `msazuresphere/AgentPlayground/_git/ado-aw-e2e-fixture`.
Its normal test refs are immutable:

| Pair | Source commits | Target commits | Purpose |
| --- | ---: | ---: | --- |
| `e2e/rest-source` / `e2e/rest-target` | 2105 | 2105 | REST-guided minimal-history path |
| `e2e/fallback-source` / `e2e/fallback-target` | 7 | 5 | bounded Git fallback |

Both pairs diverge from commit
`52cfa195595a85eedc7ad405575bd92be93fb0e3`.

Register `tests/ado-script-e2e/azure-pipelines.yml` as a GitHub-backed pipeline
in the `AgentPlayground` project. The build identity needs Code Read on the
fixture repository. The pipeline is path-filtered for relevant PR changes,
manual queues, and (after the permanent suite is complete) a daily `main`
schedule.
