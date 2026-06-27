---
name: audit-build
description: Download and analyze a finished ado-aw pipeline build. Use when the user wants a security/behavior report on a completed build — firewall/network activity, MCP tool calls, safe-output proposals, detection findings, token usage, or policy signals.
allowed-tools: Bash, Read, Glob, Grep, mcp__ado-aw__audit_build, mcp__ado-aw__trace_failure
---

# Audit a build

Analyze a **finished** ado-aw pipeline build and render its findings.

1. Confirm the `ado-aw` compiler is installed (`ado-aw --version`) and that ADO
   read auth is available (`gh`/`az` or a PAT).

2. Run the audit. Accepts a bare build id or a full Azure DevOps build URL:
   - MCP: `audit_build` — same shape as `ado-aw audit --json`; returns the
     structured `AuditData` report.
   - CLI: `ado-aw audit <build-id-or-url>` (add `--json` for machine output).
   - `trace_failure` — when the build failed, correlate the failed-job chain with
     the local IR graph.

3. The report's analyzers cover: detection-stage artifacts, AWF firewall/network
   logs, build timeline / job-level data, MCP tool calls, missing-tool /
   missing-data / noop safe outputs, OTel agent stats (token usage, duration,
   turns), policy findings (safe-output integrity, prompt-injection signals), and
   safe-output NDJSON.

4. Summarize findings by severity and call out anything in the firewall, policy,
   or safe-output sections explicitly. The audit cache is keyed on build id;
   pass `--no-cache` (CLI) / `no_cache: true` (MCP) to force a fresh download.

The user's request: $ARGUMENTS
