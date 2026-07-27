# Create an Azure DevOps Agentic Workflow (v2)

Apply `/home/runner/work/ado-aw/ado-aw/prompts/prompt-contract-v2.md` before executing this prompt.

## Core

### Role
Create one new `ado-aw` workflow source file (`.md` with YAML front matter + markdown body).

### Constraints
- Produce exactly one workflow source file unless the user asks for more.
- Prefer minimal, safe configuration.
- For default model behavior, follow compiler truth in `src/engine.rs` (`DEFAULT_COPILOT_MODEL`) instead of hardcoding assumptions.
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
- optional: `target`, `engine`, `workspace`, `pool`, `repos`, `tools`, `runtimes`, `mcp-servers`, `safe-outputs`, `on`, `steps`, `setup`, `teardown`, `permissions`, `parameters`

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

## References
- `/home/runner/work/ado-aw/ado-aw/docs/front-matter.md`
- `/home/runner/work/ado-aw/ado-aw/docs/safe-outputs.md`
- `/home/runner/work/ado-aw/ado-aw/docs/engine.md`
- `/home/runner/work/ado-aw/ado-aw/docs/targets.md`
- `/home/runner/work/ado-aw/ado-aw/docs/network.md`
