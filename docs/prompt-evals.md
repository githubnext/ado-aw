# Prompt evaluations

The prompt evaluation system protects the authoring prompts in `prompts/`:

- `create-ado-agentic-workflow.md`
- `update-ado-agentic-workflow.md`
- `debug-ado-agentic-workflow.md`

It combines a deterministic required check with an advisory pull-request
evaluator. Model-derived scores never block merging.

## Components

| Component | Purpose |
|---|---|
| `tests/prompt-evals/` | Versioned synthetic cases, context, ground truth, and rubrics |
| `scripts/prompt-evals/` | Tool-free paired runner, independent judge, and PR report renderer |
| `tests/prompt_contract_tests.rs` | Static invariants for the authoring prompts |
| `tests/prompt_eval_contract_tests.rs` | Fixture schema, path-safety, rubric, and embedded-workflow checks |
| `.github/workflows/prompt-contracts.yml` | Required, model-independent PR check |
| `.github/workflows/prompt-evaluator.md` | Advisory base-versus-candidate evaluator |

## Pull-request evaluation

The evaluator runs when a pull request changes a prompt, fixture, evaluator
script, or evaluator workflow.

It selects only the affected prompt suite unless the shared contract or
evaluator infrastructure changed. Every selected case runs twice:

1. with the pull request base prompt;
2. with the candidate prompt.

Both variants use the candidate fixture, the same ado-aw compiler, the same
subject model, the same Copilot CLI version, and the same rubric. The resulting
scores are classified as `improved`, `unchanged`, `regressed`, or
`inconclusive`.

The workflow posts one rolling PR comment and uploads the raw scorecard
artifact. It is deliberately not a required check.

## Fixture corpus

`tests/prompt-evals/manifest.json` contains the canonical case list. Each
`case.json` declares:

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
expects a workflow, the HEAD ado-aw compiler checks both variants:

```text
ado-aw compile artifact.md -o artifact.yml
ado-aw lint artifact.md --json
```

The scorecard also records response and artifact presence, required report
sections, lint errors, subject duration, and infrastructure status.

These checks are deterministic, but their sampled inputs are model-generated,
so their results remain advisory.

## Independent judging

One separate tool-free judge session scores each selected prompt suite. The
judge model is pinned in `scripts/prompt-evals/config.json` and verified against
`ado-aw catalog --kind models`.

The judge receives only the synthetic case, ground truth, deterministic
observations, subject responses, and fixed rubric. It must return strict JSON.
Every criterion receives `0`, `1`, or `2`, plus a short evidence excerpt and
reason. The harness rejects missing, duplicate, or invented criteria and
computes totals itself.

Common criteria cover task completion, grounding, safety/consent, and explicit
done criteria. Prompt-specific criteria cover workflow construction, update
minimality, or diagnostic quality.

## Reports and artifacts

`prompt-eval-results` is retained for 30 days and contains:

- `manifest.json` and `run-metadata.json`;
- `scorecard.json`;
- composed prompts and raw synthetic responses;
- extracted workflow artifacts;
- compiler and lint results;
- strict judge results;
- the rendered PR report and infrastructure exit codes.

The rolling PR comment shows per-prompt comparison counts and any
criterion-level regressions. The outer workflow agent does not score or rewrite
the report; it sends the prepared Markdown through `add-comment`.

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
2. Add `case.json`, `request.md`, and concise synthetic context files.
3. Reference the common and prompt-specific rubrics.
4. Add the case path to `manifest.json`.
5. Add or update tests for any new expected field.
6. Run the deterministic commands above.

## Limitations

- One base and one candidate sample per case are not statistical proof.
- The evaluator covers synthetic cases, not real prompt usage.
- LLM judging can still contain model bias despite strict rubrics.
- Continuous evaluation, repeated sampling, production telemetry, automatic
  prompt edits, and semantic merge gates are intentionally out of scope.
