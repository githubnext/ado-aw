# Update an Azure DevOps Agentic Workflow

Apply the shared behavior contract before executing this prompt:
https://raw.githubusercontent.com/githubnext/ado-aw/v0.45.1/prompts/prompt-contract.md <!-- x-release-please-version -->

## Core

### Role
Apply requested changes to an existing `ado-aw` workflow source file with minimal, targeted edits.

### Constraints
- Read the entire source file before editing.
- Modify only requested behavior.
- Preserve unrelated structure and intent.
- For model defaults, follow `src/engine.rs` (`DEFAULT_COPILOT_MODEL`) instead of hardcoding prompt-local defaults.
- Do not perform external side effects without explicit user consent.

### Output Format
1. Updated `.md` content.
2. Change summary by field/body section.
3. Recompile requirement and next steps.

## Task Module: Update

### 1. Baseline Current State
Identify current:
- front matter keys,
- safe-outputs + permissions,
- mcp-servers/tool allow-lists,
- trigger/schedule settings,
- instruction body structure.

### 2. Apply Targeted Changes
- Touch only requested keys/sections.
- Keep front-matter ordering stable when practical.
- If changing `on.pr` branch/path filters, ensure mode choice (`synthetic` vs `policy`) is explicitly considered.

### 3. Validate
Run a compact checklist:
- requested changes fully applied,
- no accidental privilege expansion,
- no contradictory instructions,
- no invalid safe-output references,
- recompilation need correctly determined.

Recompile rule:
- front matter changed -> `ado-aw compile` required,
- body-only changes -> compile only if `inlined-imports: true`.

### 4. Return Result
Return:
- concise diff summary,
- whether compile is required,
- next steps:
  1. review updated `.md`,
  2. if required run `ado-aw compile <path/to/file.md>`,
  3. commit changed files.

## References
- https://raw.githubusercontent.com/githubnext/ado-aw/v0.45.1/docs/front-matter.md <!-- x-release-please-version -->
- https://raw.githubusercontent.com/githubnext/ado-aw/v0.45.1/docs/runtime-imports.md <!-- x-release-please-version -->
- https://raw.githubusercontent.com/githubnext/ado-aw/v0.45.1/docs/safe-outputs.md <!-- x-release-please-version -->
- https://raw.githubusercontent.com/githubnext/ado-aw/v0.45.1/docs/engine.md <!-- x-release-please-version -->
- https://raw.githubusercontent.com/githubnext/ado-aw/v0.45.1/docs/ir.md <!-- x-release-please-version -->
