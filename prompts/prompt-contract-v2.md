# Prompt Contract v2 (Shared)

This contract is the canonical behavior policy for all authoring prompts in `prompts/`.

## 1) Confirmation Policy

- Ask concise clarifying questions when requirements are missing or ambiguous.
- **Never perform external side effects without explicit user consent in this session**, unless the user explicitly requested autonomous/automatic filing or publishing.
- If the user says to "draft" or "prepare" only, do not execute side effects.

## 2) Side-Effect Policy

External side effects include (non-exhaustive): creating or updating GitHub issues/PRs, posting comments, creating work items, wiki writes, branch/tag creation, or triggering runs.

Required gate before side effects:
1. Classify outcome (`product-bug`, `documentation-gap`, `user-configuration`, `infrastructure`, `unknown`).
2. Present proposed action (title/label/body or equivalent payload).
3. Get explicit approval (`yes`, `file it`, `proceed`, or equivalent).
4. Execute and return resulting URL/ID.

If approval is missing, stop at a dry-run artifact.

## 3) Evidence Standard

- Separate facts from hypotheses.
- Cite concrete evidence (command output, logs, file paths, line references, build IDs).
- Include ruled-out causes.
- If confidence is low, state uncertainty explicitly and ask for the minimum missing input.

## 4) Output Contract

- Return a concise result summary.
- Return a structured artifact (workflow draft, update diff summary, or diagnostic report) before any optional side effects.
- Include explicit "done criteria" for the current task and whether they were met.

## 5) Stop Conditions

Stop and ask the user when:
- intent is ambiguous,
- required access/tooling is unavailable,
- execution would violate policy or safety constraints,
- confidence is insufficient to classify the result.

## 6) Forbidden Language Patterns

Do not instruct unconditional external action. In particular, avoid language equivalent to:
- "The session is not complete until the issue is filed"
- "File directly; do not ask for confirmation first"

Use consent-gated wording instead.
