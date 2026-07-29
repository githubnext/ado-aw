# Prompt evaluations

The prompt evaluation system measures the behavior of the authoring prompts in
`prompts/`:

- `create-ado-agentic-workflow.md`
- `update-ado-agentic-workflow.md`
- `debug-ado-agentic-workflow.md`

It combines a deterministic required check with an advisory GitHub Agentic
Workflow. Model-derived scores never block merging.

## Components

| Component | Purpose |
|---|---|
| `tests/prompt-evals/` | Versioned synthetic cases, context, ground truth, and rubrics |
| `scripts/prompt-evals/` | Tool-free subject runner, independent judge, trend analysis, and report rendering |
| `tests/prompt_contract_tests.rs` | Static invariants for the authoring prompts |
| `tests/prompt_eval_contract_tests.rs` | Fixture schema, path-safety, rubric, and synthetic-data checks |
| `.github/workflows/prompt-contracts.yml` | Required, model-independent PR check |
| `.github/workflows/prompt-evaluator.md` | Advisory PR and continuous behavioral evaluator |

## Execution modes

### Pull requests

The evaluator runs when a pull request changes a prompt, fixture, evaluator
script, or evaluator workflow.

It selects only the affected prompt suite unless the shared contract or
evaluator infrastructure changed. Every selected case runs twice:

1. with the pull request base prompt;
2. with the candidate prompt.

Both variants use the candidate fixture, the same ado-aw compiler, the same
subject model, the same Copilot CLI version, and the same rubric. The resulting
base/candidate scores are classified as `improved`, `unchanged`, `regressed`,
or `inconclusive`.

The workflow posts one rolling PR comment. It is deliberately not a required
check.

### Nightly

The evaluator runs all nine cases against `main` each night. It uploads
`prompt-eval-results` with 90-day retention and compares the scorecard with
prior scheduled artifacts.

Ordinary nightly runs use `noop` after uploading the artifact. A new sustained
regression creates a Discussion immediately. The Monday run publishes the
weekly rolling Discussion, including any active alert.

### Manual

`workflow_dispatch` runs all cases for calibration. Manual runs do not enter
the sustained-run counter. Set the `publish` input to create a calibration
Discussion; otherwise the result remains artifact-only.

## Fixture corpus

`tests/prompt-evals/manifest.json` contains the canonical case list and
`fixture_set_version`.

Each `case.json` declares:

- `id` and prompt suite;
- user request and synthetic context files;
- common and suite-specific rubric files;
- expected outcome (`workflow`, `clarification`, or `diagnostic`);
- model-independent artifact, compile, lint, section, and side-effect
  expectations;
- diagnostic classification or other case ground truth.

The initial corpus has three cases per prompt:

| Prompt | Cases |
|---|---|
| Create | minimal manual workflow, scheduled safe-output workflow, underspecified request |
| Update | body-only edit, PR policy/filter edit, scoped safe-output addition |
| Debug | Stage 3 permission failure, reproducible product defect, missing evidence |

Fixtures must remain synthetic. Do not add real user conversations, Azure
DevOps logs, organization names, credentials, or work-item content.

When the meaning of a case changes, increment `fixture_set_version`. Rubric
changes require a new rubric ID such as `common-v2`.

## Subject isolation

The subject runner uses the model and Copilot CLI version pinned by
`src/engine.rs`. It starts a fresh non-interactive session for every case and
variant.

The session:

- receives the shared contract, task prompt, user request, and synthetic
  context inline;
- has an empty available-tool set;
- disables built-in MCPs, custom instructions, memory, asking the user, remote
  control, and remote export;
- receives no `GITHUB_TOKEN`, `GH_TOKEN`, Azure DevOps token, or safe-output
  credential;
- retains only `COPILOT_GITHUB_TOKEN` for model authentication;
- stores verbose CLI logs outside the uploaded artifact.

Subject concurrency is bounded by `scripts/prompt-evals/config.json`.

## Deterministic observations

Workflow-shaped responses are extracted from Markdown fences. When a case
expects a workflow, the HEAD ado-aw compiler runs the same checks for every
variant:

```text
ado-aw compile artifact.md -o artifact.yml
ado-aw lint artifact.md --json
```

The scorecard also records:

- response and artifact presence;
- required report sections;
- compiler and lint exit status;
- lint error count;
- subject duration and infrastructure status.

These checks are deterministic, but their sampled inputs are model-generated,
so their PR results remain advisory.

## Independent judging

One separate tool-free judge session scores each selected prompt suite. The
judge model is pinned in `scripts/prompt-evals/config.json` and verified against
`ado-aw catalog --kind models`.

The judge receives only:

- the synthetic case and ground truth;
- deterministic observations;
- subject responses;
- the fixed rubric.

It must return strict JSON. Every criterion receives `0`, `1`, or `2`, plus a
short evidence excerpt and reason. The harness rejects missing, duplicate, or
invented criteria and computes totals itself.

Common criteria cover task completion, grounding, safety/consent, and explicit
done criteria. Prompt-specific criteria cover workflow construction, update
minimality, or diagnostic quality.

## Continuous trends

Scheduled scorecards are comparable only when all cohort inputs match:

- fixture-set and case-content digests;
- rubric digest;
- evaluator script/config/workflow digest;
- subject model;
- judge model;
- Copilot CLI version.

Prompt commit and prompt digest are metadata, not cohort keys, because prompt
changes are what the system is intended to detect.

Infrastructure-incomplete or inconclusive runs do not participate in alert
calculations.

The evaluator computes:

- normalized rubric score;
- workflow artifact extraction rate;
- compile success rate;
- error-free lint rate;
- safety/consent pass rate;
- inconclusive and infrastructure-failure rates;
- subject duration.

### Sustained regression threshold

Alerting requires at least seven preceding comparable runs.

A semantic alert starts only when:

1. all three latest comparable nightly runs are at least 10 normalized
   percentage points below the median of up to the preceding 14 runs; and
2. at least two cases show the same sustained decline.

A hard-observable alert starts when the same artifact extraction, compile, or
error-free lint check fails for the same case in all three latest runs after a
baseline success rate of at least 80%.

Only the false-to-true alert transition creates an immediate Discussion.
Already-active alerts are not reposted nightly. Model, CLI, fixture, or rubric
changes begin a new cohort and cannot trigger an alert across the boundary.

## Reports and artifacts

`prompt-eval-results` contains:

- `manifest.json` and `run-metadata.json`;
- `scorecard.json`;
- `trend.json` for scheduled/manual runs;
- composed prompts and raw synthetic responses;
- extracted workflow artifacts;
- compiler and lint results;
- strict judge results;
- rendered report files and infrastructure exit codes.

The PR comment shows per-prompt comparison counts and any criterion-level
regressions. The weekly or alert Discussion shows seven-run trends, the previous
seven-run comparison, active alert evidence, cohort metadata, and evaluator
infrastructure health.

The outer workflow agent does not score or rewrite the report. It reads
`report-context.json` and dispatches exactly one prepared safe output:

- `add-comment`;
- `create-discussion`; or
- `noop`.

All implicit issue fallbacks, failure issues, missing-tool issues,
report-incomplete issues, activation comments, and noop tracking issues are
disabled.

## Required deterministic check

`Prompt Contracts` runs:

```text
cargo test --test prompt_contract_tests --test prompt_eval_contract_tests
node --test scripts/prompt-evals/test/*.test.mjs
gh aw compile prompt-evaluator --strict
```

It also fails when the generated `prompt-evaluator.lock.yml` is stale. Configure
`Prompt Contracts / Prompt Contracts` as a required branch-protection check.
Do not make the behavioral `Prompt Evaluator` workflow required.

## Adding a case

1. Add a case directory under `tests/prompt-evals/cases/<prompt>/`.
2. Add `case.json`, `request.md`, and any concise synthetic context files.
3. Reference the common and prompt-specific rubrics.
4. Add the case path to `manifest.json`.
5. Increment `fixture_set_version`.
6. Add or update tests for any new expected field.
7. Run the deterministic commands above.

## Limitations

- One sample per case per night is a monitoring signal, not statistical proof.
- Continuous evaluation covers synthetic cases, not real prompt usage.
- Artifact history is bounded by repository retention policy.
- Within-run repeated sampling, production telemetry, automatic prompt edits,
  and semantic merge gates are intentionally out of scope.
