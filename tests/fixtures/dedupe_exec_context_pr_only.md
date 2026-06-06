---
name: "Dedupe Exec Context PR Only"
description: "Gate inactive (no filters), runtime imports inlined, but on.pr is configured so the execution-context PR contributor activates — bundle download must land in Agent only"
inlined-imports: true
on:
  pr:
    branches:
      include: [main]
---

## Dedupe Exec Context PR Only

Used by `test_exec_context_pr_only_downloads_bundle_in_agent_job_not_setup`.
Closes a coverage gap left by `dedupe_gate_only.md` pinning
`execution-context.pr.enabled: false`.
