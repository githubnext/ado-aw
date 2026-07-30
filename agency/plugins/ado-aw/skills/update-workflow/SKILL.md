---
name: update-workflow
description: Update an existing ado-aw agentic Azure DevOps pipeline. Use when the user wants to modify an existing Markdown agent file — change schedule, add a tool/MCP, add a safe output, adjust runtimes, etc. Read-then-update with validation.
allowed-tools: Bash, Read, Write, Edit, Glob, Grep, mcp__ado-aw__inspect_workflow, mcp__ado-aw__graph_summary, mcp__ado-aw__step_dependencies, mcp__ado-aw__whatif, mcp__ado-aw__lint_workflow, mcp__ado-aw__catalog
---

# Update an ado-aw workflow

You are modifying an **existing** ado-aw agentic workflow.

1. Confirm the `ado-aw` compiler is installed (`ado-aw --version`).

2. **Read the existing agent file first**, and inspect its current shape before
   changing anything: `inspect_workflow`, `graph_summary`, `step_dependencies`,
   and `whatif` (to understand downstream impact of a change).

3. Load the **entire** content of the authoritative, version-pinned playbook and
   follow its instructions precisely:

   https://raw.githubusercontent.com/githubnext/ado-aw/v0.46.0/prompts/update-ado-agentic-workflow.md <!-- x-release-please-version -->

4. Use `catalog` to confirm any newly referenced safe-output / runtime / tool /
   model identifiers are valid.

5. **Recompile** with `ado-aw compile` after editing YAML front matter, then run
   `lint_workflow` until clean. Markdown-body-only changes do not require
   recompilation.

The user's request: $ARGUMENTS
