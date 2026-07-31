# Debug an Azure DevOps Agentic Pipeline

Apply `prompts/prompt-contract.md` before executing this prompt.

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
Use `ado-aw` CLI tools as the primary investigation path:
- Run `ado-aw audit <build-id-or-url>` to download the three artifact families (agent outputs, detection verdict, safe outputs) and run the built-in analyzers in one step. This is faster than manually trawling ADO logs.
- Run `ado-aw trace <build-id-or-url>` to correlate the build's telemetry with the local typed-IR graph and explain failed-job chains and downstream skip classifications.
- Run `ado-aw graph dump <source.md>` to visualise the pipeline's job/step dependency graph and understand structural relationships.

Supplement with:
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
- `docs/audit.md`
- `docs/ir.md`
- `docs/mcp-author.md`
- `docs/safe-outputs.md`
