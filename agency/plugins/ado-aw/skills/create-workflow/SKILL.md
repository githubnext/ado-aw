---
name: create-workflow
description: Create a new ado-aw agentic Azure DevOps pipeline from scratch. Use when the user wants to author a brand-new AI-powered ADO pipeline as a Markdown agent file. Ends with a clean compile + lint.
allowed-tools: Bash, Read, Write, Edit, Glob, Grep, mcp__ado-aw__catalog, mcp__ado-aw__lint_workflow, mcp__ado-aw__inspect_workflow, mcp__ado-aw__graph_summary
---

# Create an ado-aw workflow

You are creating a **new** ado-aw agentic workflow.

1. Confirm the `ado-aw` compiler is installed (`ado-aw --version`). If not,
   install it using the platform installer scripts from the ado-aw releases page
   (see the plugin README / `scripts/doctor.*`).

2. Load the **entire** content of the authoritative, version-pinned playbook and
   follow its instructions precisely:

   https://raw.githubusercontent.com/githubnext/ado-aw/v0.40.0/prompts/create-ado-agentic-workflow.md <!-- x-release-please-version -->

3. While authoring, use the read-only MCP tools to stay grounded:
   - `catalog` — discover available safe-outputs, runtimes, tools, engines, models.
   - `lint_workflow` — structural lint checks on the draft.
   - `inspect_workflow` / `graph_summary` — confirm the compiled IR/graph shape.

4. **Finish only when** `ado-aw compile <file>.md` succeeds and `lint_workflow`
   is clean. Recompile after any YAML front-matter change.

The user's request: $ARGUMENTS
