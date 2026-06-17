# Self-optimization

_Part of the [ado-aw documentation](../AGENTS.md)._

`self-optimization:` is an opt-in front-matter feature that lets a Stage-1
agent propose moving small, deterministic, repeatedly executed bash work out of
its prompt body and into front-matter step sections. Typical candidates are
clone/fetch, cache restore, runtime install, and artifact download steps that
do not depend on agent reasoning.

The feature is conservative by design. It is **off by default**, **staged by
default**, and constrained by two independent safety checks before any source
file mutation can land: Stage 2 cross-checks the proposal against the agent's
actual command history, and Stage 3 IR-validates the proposed step block with a
curated allow-list. By default, proposals may only target `steps:` and
`post-steps:` — the two sections that run in the same job and security context
as the agent.

The intended adoption path is: enable the feature, review staged previews in
the SafeOutputs build log for a few runs, then flip `staged: false` only after
the proposals are consistently correct for that workflow.

## Configuration

```yaml
self-optimization:
  enabled: true              # opt-in (default: false)
  staged: true               # preview-only (default: true)
  max-proposals-per-run: 3   # cap (default: 3, max: 50)
  allowed-sections:          # which sections proposals may target
    - steps                  # (default: [steps, post-steps])
    - post-steps
    # Uncomment to allow proposals targeting separate jobs:
    # - setup
    # - teardown
```

### Fields

- **`enabled`** (`bool`, default `false`) — master switch. When `true`, the
  compiler enables the `propose-step-optimization` MCP tool for Stage 1 and
  passes the resolved config to Stage 3.
- **`staged`** (`bool`, default `true`) — preview mode. When `true`, Stage 3
  prints an `🎭 Proposed Step Optimization` preview to the SafeOutputs build
  log and does not mutate the source file. When `false`, Stage 3 opens a PR
  against the source `.md`.
- **`max-proposals-per-run`** (`u32`, default `3`, maximum `50`) — per-run cap.
  Values above `50` are clamped during config sanitization.
- **`allowed-sections`** (`[steps, post-steps]` by default) — front-matter
  sections proposals may target. `setup` and `teardown` are valid values, but
  they are intentionally not in the default because they run in separate jobs.

## How it works

```text
Stage 1 agent
  └─ calls propose-step-optimization
       (section, rationale, estimated_token_savings, steps, source_command_evidence)
          ↓
safe_outputs.ndjson
          ↓
Stage 2 detection
  └─ cross-checks proposed bash against source_command_evidence and the
     agent's actual command history
          ↓
Stage 3 executor
  ├─ checks section ∈ allowed-sections
  ├─ IR-validates steps with StepKindAllow::Curated
  └─ staged: true  → build-log preview
     staged: false → source-file patch + branch push + PR
```

1. **Stage 1: proposal**
   - When `self-optimization.enabled: true` is set, the Stage-1 agent gets the
     `propose-step-optimization` MCP tool.
   - The tool records a structured proposal containing:
     - `section`
     - `rationale`
     - `estimated_token_savings`
     - `steps` (JSON array of ADO step entries)
     - `source_command_evidence` (bash commands the agent says it actually ran)

2. **Stage 2: detection cross-check**
   - The detection prompt treats self-optimization proposals as grounded only if
     the proposed bash matches both `source_command_evidence` and the agent's
     actual command history.
   - Proposed bash that the agent did not demonstrably run is treated as a
     prompt-injection signal and should be flagged as such.
   - Proposals targeting `setup` or `teardown` get extra scrutiny because they
     affect separate jobs.

3. **Stage 3: validation and application**
   - Stage 3 rejects proposals whose `section` is not listed in
     `self-optimization.allowed-sections`.
   - It then runs the shared IR validator with `StepKindAllow::Curated`.
   - If validation succeeds:
     - **staged mode** renders a preview only.
     - **live mode** patches the source markdown front matter, pushes a commit
       to a new branch, and opens a PR via Azure DevOps REST.

## Staged mode

`staged: true` is the default and recommended starting point. In this mode,
Stage 3 does not edit the source file. Instead it prints a preview like this in
the SafeOutputs build log:

````text
═══════════════════════════════════════════════════════════════════
🎭 Proposed Step Optimization (staged preview — no changes applied)
═══════════════════════════════════════════════════════════════════

Section: `steps`
Rationale: Lift deterministic clone/install work out of the prompt body
Estimated token savings: ~4200 tokens/build

Proposed YAML (add to `steps:` in your agent .md):
```yaml
- bash: git fetch --depth=1 origin main
  displayName: Fetch main
```

To apply this optimization, set `self-optimization.staged: false`
in your front matter and the next build will open a PR.
═══════════════════════════════════════════════════════════════════
````

Use staged mode to answer three questions:

- Is the work actually deterministic and repeated on every run?
- Is the target section correct (`steps:` vs `post-steps:`)?
- Is the proposed YAML something you would accept in a normal author-edited PR?

Flip `staged: false` only after the previews are consistently useful for that
workflow.

## Live mode

When `staged: false`, Stage 3 applies the same section check and curated IR
validation, then edits the source `.md`, pushes a new branch, and opens a PR.

### Branch naming

Live-mode branches use this format:

```text
ado-aw/self-opt-<section>-<8-hex>
```

Examples:

```text
ado-aw/self-opt-steps-a1b2c3d4
ado-aw/self-opt-post-steps-0f4e9c2a
```

### PR shape

- **PR title:** `chore(ado-aw): self-optimize \`<section>\` steps`
- **PR body:** includes the target section, rationale, optional estimated token
  savings, and a note that the proposal passed curated IR validation and the
  Stage 2 detection cross-check.

Example body:

```markdown
## Self-Optimization Proposal

**Section:** `steps`
**Rationale:** Lift deterministic clone/install work out of the prompt body

Estimated token savings: ~4200 tokens/build.

This PR was automatically opened by the `propose-step-optimization`
safe-output (self-optimization live mode). The proposed steps passed
IR validation (Curated allow-list: bash + typed-factory tasks only)
and the Stage 2 detection cross-check.

Review the YAML changes and merge if they look correct.
```

Implementation notes:

- The front matter is patched by parsing YAML, inserting or extending the target
  section, and re-serializing it.
- Because the YAML is re-serialized, front-matter comments and formatting may be
  normalized in the PR diff.

## Allowed step kinds

Self-optimization proposals are validated in **Curated** mode, not the more
permissive authoring mode used by `validate_steps` with `allow_list: "full"`.

Curated mode accepts:

- **`bash:`** steps
- **`task:`** steps whose task id is in
  `src/compile/ir/tasks.rs::CURATED_TASK_IDS`

Today that list is:

- `ArchiveFiles@2`
- `CopyFiles@2`
- `DockerInstaller@0`
- `DotNetCoreCLI@2`
- `PublishTestResults@2`

Arbitrary `task:` identifiers are rejected, even if Azure DevOps would accept
them syntactically. The allow-list is intentionally tied to typed task
factories. To grow it, add a typed factory and update the curated list; see
[`docs/extending.md`](extending.md).

## Security model

- **Explicit opt-in** — the tool is not exposed unless
  `self-optimization.enabled: true` is present in front matter.
- **Stage 2 grounding check** — detection compares proposed bash with both
  `params.source_command_evidence` and the recorded command history. A proposal
  that claims the agent ran `X` but did not is treated as a prompt-injection
  candidate.
- **Curated IR validation** — Stage 3 accepts only `bash:` and curated typed
  tasks. Arbitrary `TaskStep`s are rejected to keep the untrusted surface small
  and explicit.
- **Section allow-list** — `steps` and `post-steps` are the defaults because
  they run in the same job and identity context as the agent. `setup` and
  `teardown` require explicit opt-in because they are separate jobs and may run
  under different identities.
- **No `safe-outputs.propose-step-optimization` backdoor** — the compiler
  rejects this tool under `safe-outputs:`. The top-level
  `self-optimization:` section is the only supported activation path.

## Hoist candidates heuristic

The feature is meant for work that satisfies all three of these tests:

1. **Deterministic** — no agent reasoning is required.
2. **Every invocation** — it happens on essentially every run.
3. **Fixed inputs** — repo URL, branch, version, cache key, or artifact name is
   known ahead of time.

If the answer is "yes" to all three, the work is usually a good hoist
candidate. Common examples:

- clone/fetch of a known repository or branch
- restoring a known cache
- installing a pinned tool or runtime
- downloading a known artifact before or after the agent runs

Work that depends on the agent's decision-making is usually **not** a hoist
candidate. For authoring guidance that uses the same heuristic, see:

- [`prompts/create-ado-agentic-workflow.md`](../prompts/create-ado-agentic-workflow.md)
- [`prompts/update-ado-agentic-workflow.md`](../prompts/update-ado-agentic-workflow.md)
- [`prompts/debug-ado-agentic-workflow.md`](../prompts/debug-ado-agentic-workflow.md)

## Troubleshooting

| Problem | Meaning | What to do |
| --- | --- | --- |
| `Proposed steps failed IR validation` | The proposed YAML was not a valid curated step block. Common causes are invalid step shape, unsupported keys, non-string `bash`, or a `task:` id outside `CURATED_TASK_IDS`. | Reproduce locally with [`validate_steps`](mcp-author.md#validate_steps), preferably using `allow_list: "curated"` for the proposed block, then fix the YAML. |
| `Section \`...\` is not in the self-optimization allowed-sections list` | The proposal targeted a section that the workflow did not opt into. | Add the section to `self-optimization.allowed-sections`, or retarget the proposal to `steps` / `post-steps`. |
| `Failed to push self-optimization commit` or `failed to create PR` | Live mode reached the write path, but Stage 3 could not push the branch or open the PR. | Check repo write permissions and token scope for the SafeOutputs job. See [`docs/safe-output-permissions.md`](safe-output-permissions.md). If the branch push succeeded but PR creation failed, the error includes the pushed branch name for manual recovery. |

## Related

- [`docs/front-matter.md`](front-matter.md#self-optimization-opt-in) — front-matter
  syntax and the short overview entry.
- [`docs/safe-outputs.md`](safe-outputs.md#propose-step-optimization-opt-in) —
  the `propose-step-optimization` safe-output entry under self-modification.
- [`docs/mcp-author.md`](mcp-author.md#validate_steps) — the author/debug MCP
  `validate_steps` tool.
- [`docs/extending.md`](extending.md#validating-untrusted-step-blocks) — the
  shared IR validator entry point and curated/full allow-list guidance.
