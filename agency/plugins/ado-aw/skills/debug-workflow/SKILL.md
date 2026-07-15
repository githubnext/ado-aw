---
name: debug-workflow
description: Diagnose a failing ado-aw agentic Azure DevOps pipeline. Use when the user reports a pipeline failing, an agent unable to reach an MCP/network host, safe outputs not being applied, or wants to understand why a build behaved a certain way.
allowed-tools: Bash, Read, Write, Edit, Glob, Grep, mcp__ado-aw__trace_failure, mcp__ado-aw__audit_build, mcp__ado-aw__whatif, mcp__ado-aw__inspect_workflow, mcp__ado-aw__graph_summary, mcp__ado-aw__lint_workflow
---

# Debug an ado-aw workflow

You are troubleshooting a **failing** ado-aw agentic workflow.

1. Confirm the `ado-aw` compiler is installed (`ado-aw --version`).

2. Gather evidence before theorizing:
   - `trace_failure` — trace the build's failed-job chain using audit data plus
     the local IR graph (pass the build id or full ADO build URL).
   - `audit_build` — download and analyze the build's artifacts (firewall/network
     logs, MCP tool calls, safe-output NDJSON, detection findings, policy signals).
   - `whatif` — classify which downstream jobs are skipped when a given step/job
     fails, to confirm the blast radius.

3. Load the **entire** content of the authoritative, version-pinned playbook and
   follow its instructions precisely:

   https://raw.githubusercontent.com/githubnext/ado-aw/v0.45.0/prompts/debug-ado-agentic-workflow.md <!-- x-release-please-version -->

4. For Stage 3 (SafeOutputs) 401/403 failures, consult
   `docs/safe-output-permissions.md` and surface the auth/permission error
   verbatim with the doc pointer.

5. If a fix requires a source change, validate it with `ado-aw compile` +
   `lint_workflow` before proposing it.

The user's request: $ARGUMENTS
