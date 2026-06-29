---
name: compile-and-validate
description: Compile an ado-aw agent file to Azure DevOps pipeline YAML and validate it. Use when the user asks to compile a workflow, check whether it is valid, verify a pipeline matches its source, or understand the compiled IR / dependency graph.
allowed-tools: Bash, Read, Glob, Grep, mcp__ado-aw__lint_workflow, mcp__ado-aw__inspect_workflow, mcp__ado-aw__graph_summary, mcp__ado-aw__graph_dump, mcp__ado-aw__step_dependencies, mcp__ado-aw__step_outputs
---

# Compile and validate

Compile and validate an ado-aw agentic workflow. This skill is CLI- and
MCP-driven; it does not fetch a remote playbook.

1. Confirm the `ado-aw` compiler is installed (`ado-aw --version`).

2. **Compile** with the CLI (mutates the `.lock.yml` on disk):
   - `ado-aw compile <agent-file.md>` — compile one file.
   - `ado-aw compile` — auto-discover and recompile all detected pipelines.
   - `ado-aw check <pipeline.lock.yml>` — verify a compiled pipeline still
     matches its source Markdown (good for CI parity checks).

3. **Validate and explain** with the read-only MCP tools (no mutation):
   - `lint_workflow` — structural lint checks; resolve every finding.
   - `inspect_workflow` — the public `PipelineSummary` (schema_version = 1).
   - `graph_summary` / `graph_dump` — the resolved dependency graph
     (text or Graphviz DOT).
   - `step_dependencies` — upstream/downstream traversal for a step or job id.
   - `step_outputs` — declared outputs and their consumers.

4. Treat the workflow as "valid" only when `compile` succeeds **and**
   `lint_workflow` is clean. Recompile after any YAML front-matter change.

The user's request: $ARGUMENTS
