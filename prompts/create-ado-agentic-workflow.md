# Create an Azure DevOps Agentic Workflow

Apply the shared prompt contract before executing this prompt: https://raw.githubusercontent.com/githubnext/ado-aw/main/prompts/prompt-contract.md

## Core

### Role
Create one new `ado-aw` workflow source file (`.md` with YAML front matter + markdown body).

### Constraints
- Produce exactly one workflow source file unless the user asks for more.
- Prefer minimal, safe configuration.
- For default model behavior, follow compiler truth in `src/engine.rs`: ado-aw currently has no compiler-selected default and omits `--model` unless `engine.model` is configured.
- Do not perform external side effects unless the user explicitly asks.

### Output Format
1. Final workflow markdown.
2. Short assumptions list.
3. Recompile guidance (`ado-aw compile <path>`).

## Task Module: Create

### 1. Gather Required Inputs
Collect or infer:
- workflow name
- one-line description
- primary task objective
- trigger mode (manual/schedule/pr/pipeline)
- repositories/workspace scope

If interactive, ask only missing essentials first.

### 2. Build Front Matter
Use only required keys plus task-required options:
- `name`, `description`
- optional: `target`, `engine`, `workspace`, `pool`, `repos`, `imports`, `tools`, `runtimes`, `mcp-servers`, `safe-outputs`, `on`, `steps`, `post-steps`, `setup`, `teardown`, `permissions`, `permissions-required`, `parameters`, `env`, `variable-groups`, `network`, `execution-context`, `inlined-imports`, `supply-chain`

Rules:
- Omit fields that equal defaults.
- Keep least-privilege permissions.
- Keep MCP and safe-output allow-lists narrow.

### 3. Build Agent Body
Use compact sections:
- `## Objective`
- `## Inputs`
- `## Procedure`
- `## Output`
- `## No Action`

Ensure "No Action" explicitly maps to `noop` when applicable.

For Azure DevOps PR work, choose tools by intent:
- Ad hoc general/inline feedback: `add-pull-request-comment`; replies use
  `reply-to-pull-request-comment` with an existing thread ID.
- A complete review: one `submit-pull-request-review` proposal containing
  `event`, optional `body` and `comments`. Standalone comments are never buffered
  into it; do not submit the same finding through both routes.
- `comment` is non-voting; `reset` explicitly clears the authenticated actor's
  vote. Resolving a thread does not approve a PR or clear a vote.
- Inline findings require `expected_head_sha`; nested review findings require
  explicit `max-comments` (default 0). New mutation tools default to the complete
  trusted triggering PR; arbitrary PR IDs require `target: "*"`.
- Edit PR text with `update-pull-request`; edit only a verified owned comment
  with `update-pull-request-comment` and both thread/comment IDs.
- Code repairs use `push-to-pull-request-branch`, an explicit source-branch
  allowlist and the original source-head snapshot, never an arbitrary git push.
  Draft publication is separate from voting and auto-complete.

All tools enqueue proposals in Stage 1; none publishes immediately. A complete
review can require several non-atomic Stage 3 writes. Consult `docs/safe-outputs.md`
for ownership, partial-outcome, approval and same-lane constraints.

### 4. Validate Draft Quality
Checklist:
- field set is minimal and coherent,
- side effects route through safe-outputs,
- permissions are least privilege,
- trigger semantics match user intent,
- instructions are deterministic and concise.

### 5. Return Result
Return:
- complete `.md` content,
- assumptions and unresolved questions,
- next steps:
  1. save file,
  2. run `ado-aw compile <path/to/file.md>`,
  3. commit both `.md` and generated `.lock.yml`.

## Done Criteria
- Exactly one complete workflow source is returned when required inputs are known.
- The draft passes the quality checklist and does not introduce unrequested privilege.
- Assumptions, unresolved inputs, and compilation guidance are explicit.
- When essential inputs are missing, concise clarification is returned instead of an invented workflow.

## References
- https://raw.githubusercontent.com/githubnext/ado-aw/main/docs/front-matter.md
- https://raw.githubusercontent.com/githubnext/ado-aw/main/docs/safe-outputs.md
- https://raw.githubusercontent.com/githubnext/ado-aw/main/docs/engine.md
- https://raw.githubusercontent.com/githubnext/ado-aw/main/docs/targets.md
- https://raw.githubusercontent.com/githubnext/ado-aw/main/docs/network.md
