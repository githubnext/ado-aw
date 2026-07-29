# Prompt evaluation fixtures

This directory contains synthetic cases for evaluating the authoring prompts in
`prompts/`.

`manifest.json` is the canonical case list. Every case file:

- uses schema version 1;
- has a globally unique ID;
- identifies one prompt suite (`create`, `update`, or `debug`);
- references request and context files relative to the case directory;
- references rubric files relative to this directory;
- declares the expected outcome and model-independent observables.

All content must be synthetic. Do not copy real build logs, organization names,
credentials, work-item content, or user conversations into these fixtures.

The advisory Prompt Evaluator workflow uses one gh-aw-managed agent run to
review base and candidate prompt text against these cases and rubrics. It does
not invoke Copilot CLI directly or execute the authoring prompts.

When adding or changing a case:

1. Update `manifest.json`.
2. Keep request and context data concise.
3. Add explicit ground truth for diagnostic cases.
4. Update `fixture_set_version` when case meaning changes.
5. Run:

   ```text
   cargo test --test prompt_contract_tests --test prompt_eval_contract_tests
   gh aw compile prompt-evaluator --strict
   ```
