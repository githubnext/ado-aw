# Debug an Azure DevOps Agentic Pipeline

Apply the shared prompt contract before executing this prompt: https://raw.githubusercontent.com/githubnext/ado-aw/main/prompts/prompt-contract.md

## Core

### Role
Diagnose why an `ado-aw` pipeline run failed and produce a structured diagnostic report.

### Default Mode
**Dry-run report only.** Filing external issues is optional and requires explicit user approval in this session.

### Constraints
- Focus on diagnosis, not code changes.
- Separate evidence from inference.
- If confidence is low, ask for minimal missing data.
- Do not file external issues unless consent gate passes.

### Output Format
1. Diagnostic summary
2. Evidence
3. Root-cause classification
4. Proposed follow-up action (optional)

## Task Module: Debug

### Step 1 — Establish Target Run
Collect build context:
- org/project/repo,
- pipeline definition + run/build ID,
- failure time window,
- last known successful run (if available).

### Step 2 — Investigate
Use available tools/logs to gather:
- timeline state by stage/job,
- failing job logs,
- safe-output and detection artifacts when present,
- recent config/version drift relevant to the failure.

### Step 3 — Classify
Assign one class:
- `product-bug`
- `documentation-gap`
- `user-configuration`
- `infrastructure`
- `unknown`

Include confidence (`high|medium|low`) and ruled-out alternatives.

### Step 4 — Produce Diagnostic Report
Minimum sections:
- `## Diagnostic Summary`
- `## Evidence`
- `## Analysis`
- `## Root Cause`
- `## Recommended Next Action`

### Step 5 — Consent-Gated Filing (Optional)
Default behavior: **do not file**.

If and only if the user explicitly asks to file now (or explicitly enabled auto-file in this session), then:
1. Propose issue title/label/body from the report.
2. Confirm approval.
3. File and return URL.

If consent is absent, end with report + ready-to-file issue draft.

## Decision Table

| Condition | Action |
|---|---|
| Missing evidence prevents confident classification | Ask for missing inputs; do not file |
| Classified as `user-configuration` with medium/high confidence | Provide remediation-focused report draft; do not auto-file |
| Classified as `product-bug` or `documentation-gap` and user approves filing | File issue and return URL |
| Any case without explicit approval | Return dry-run report/draft only |

## Done Criteria
- Complete when diagnostic report is delivered.
- Filing is complete only when explicitly approved and URL is returned.

## References
- https://raw.githubusercontent.com/githubnext/ado-aw/main/docs/audit.md
- https://raw.githubusercontent.com/githubnext/ado-aw/main/docs/ir.md
- https://raw.githubusercontent.com/githubnext/ado-aw/main/docs/mcp-author.md
- https://raw.githubusercontent.com/githubnext/ado-aw/main/docs/safe-outputs.md
