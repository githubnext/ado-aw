# Prompt evaluations

The prompt evaluation system protects the authoring prompts in `prompts/`:

- `create-ado-agentic-workflow.md`
- `update-ado-agentic-workflow.md`
- `debug-ado-agentic-workflow.md`

It combines a deterministic required check with an advisory, static
base-versus-head review performed by a single gh-aw-managed agent run.

## Components

| Component | Purpose |
|---|---|
| `tests/prompt-evals/` | Versioned synthetic cases, context, ground truth, and rubrics |
| `tests/prompt_contract_tests.rs` | Static invariants for the authoring prompts |
| `tests/prompt_eval_contract_tests.rs` | Fixture schema, path-safety, rubric, and embedded-workflow checks |
| `.github/workflows/prompt-contracts.yml` | Required, model-independent PR check |
| `.github/workflows/prompt-evaluator.md` | Advisory prompt-change reviewer |

## Deterministic Prompt Contracts

`Prompt Contracts` runs:

```text
cargo test --test prompt_contract_tests --test prompt_eval_contract_tests
gh aw compile prompt-evaluator --strict
```

It checks prompt policy wording, fixture structure, synthetic-data constraints,
rubric completeness, embedded workflow validity, and generated lock-file
freshness.

Configure `Prompt Contracts / Prompt Contracts` as a required
branch-protection check.

## Advisory prompt review

The Prompt Evaluator runs when a pull request changes a prompt, fixture,
contract test, or the evaluator workflow.

A deterministic preparation step fetches the base and head commits and stages:

- the changed path list;
- base versions of all four prompt files;
- head versions of all four prompt files;
- the head version of `tests/prompt-evals/`.

These files are placed under `/tmp/gh-aw/agent/prompt-evals/` and become input
to the normal gh-aw agent execution.

The workflow does **not** invoke Copilot CLI directly, start a separate subject
model, run a judge model, or use subagents. gh-aw owns the single Copilot engine
invocation.

The first complete nine-case run used 44.2 AI credits for evaluation and 6.54
for threat detection. The workflow therefore uses soft caps of 100 and 50
respectively, with a 500-credit daily cap to allow several PR revisions. There
are no per-case or judge-session budgets.

The agent:

1. selects the affected create, update, and/or debug suites;
2. reads each selected synthetic case and its fixed rubrics;
3. reviews base and candidate prompt instructions independently;
4. scores every criterion `0`, `1`, or `2` using its supplied anchors;
5. cites prompt text supporting each score;
6. classifies the candidate as improved, unchanged, regressed, or
   inconclusive;
7. posts one rolling advisory PR comment.

This is static semantic review. It does not execute the authoring prompts and
must not claim that a generated workflow compiled or that a diagnosis ran.

The workflow is intentionally non-required because its conclusions are
model-derived.

## Fixture corpus

`tests/prompt-evals/manifest.json` contains the canonical case list. Each
`case.json` declares:

- `id` and prompt suite;
- user request and synthetic context files;
- common and suite-specific rubric files;
- expected outcome (`workflow`, `clarification`, or `diagnostic`);
- model-independent expectations;
- diagnostic classification or other case ground truth.

The initial corpus has three cases per prompt:

| Prompt | Cases |
|---|---|
| Create | minimal manual workflow, scheduled safe-output workflow, underspecified request |
| Update | body-only edit, PR policy/filter edit, scoped safe-output addition |
| Debug | Stage 3 permission failure, reproducible product defect, missing evidence |

Fixtures must remain synthetic. Do not add real user conversations, Azure
DevOps logs, organization names, credentials, or work-item content.

## Security and side effects

The agent receives read-only repository and pull-request permissions. Visible
writes are limited to one `add-comment` safe output.

The workflow disables:

- file editing;
- issue creation;
- missing-tool and report-incomplete issue fallbacks;
- workflow-failure issues;
- activation comments;
- noop tracking issues;
- mentions and GitHub reference backlinks.

Fixture content is treated as untrusted data and cannot replace the evaluator
task.

## Adding a case

1. Add a case directory under `tests/prompt-evals/cases/<prompt>/`.
2. Add `case.json`, `request.md`, and concise synthetic context files.
3. Reference the common and prompt-specific rubrics.
4. Add the case path to `manifest.json`.
5. Add or update tests for any new expected field.
6. Run the deterministic commands above.

## Limitations

- Static review estimates likely prompt behavior; it does not execute prompts.
- One model review is not statistical proof.
- The evaluator covers synthetic cases, not real prompt usage.
- Continuous evaluation, repeated sampling, explicit judge models, production
  telemetry, automatic prompt edits, and semantic merge gates are out of
  scope.
