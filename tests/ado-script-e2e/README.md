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

The registered definition is `\ado-script-e2e\ado-script e2e` (ID `2544`) in
the `AgentPlayground` project. It uses Microsoft-hosted Ubuntu because the test
needs only Node and Git; it does not depend on the internal executor E2E pool.
The build identity needs Code Read on the fixture repository and each resource
checkout must be authorized for the definition.

Coverage:

- `ExplicitFullHistory` proves YAML `fetchDepth: 0` overrides the pipeline
  setting and produces a non-shallow checkout.
- `RestGuidedShallow` proves a 2105/2105 divergence resolves through ADO
  `commonCommit` / ahead / behind metadata while the checkout remains shallow.
- `BoundedFallbackShallow` disables REST and exercises the 200/500/2000
  dual-ref fallback.
- `TargetWorktreeShallow` proves the SafeOutputs mode fetches only the target
  tip and does not make the merge-base locally reachable.

The pre-fix baseline is build
[`623611`](https://dev.azure.com/msazuresphere/AgentPlayground/_build/results?buildId=623611).
It records Azure's depth-1 checkout and the target-only bundle warning before
failing with an unresolved merge-base.

The pipeline is path-filtered for relevant GitHub PR changes, manually
queueable, and scheduled daily on `main`. Because excluded PRs receive no
status, it is intentionally not configured as a global required GitHub check.

Although this pipeline does not consume the candidate-smoke write connection,
it is a GitHub-backed AgentPlayground PR definition with protected repository
access. Its live ADO trigger must keep fork builds and fork secrets disabled;
definition `2544` is included in
[`tests/compiler-smoke-e2e/trigger-policy.json`](../compiler-smoke-e2e/trigger-policy.json)
and the trusted candidate-smoke audit.
