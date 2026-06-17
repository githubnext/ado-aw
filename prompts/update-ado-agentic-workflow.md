# Update an Azure DevOps Agentic Workflow

Modify an existing **ado-aw** agent file — a markdown document with YAML front matter that the `ado-aw` compiler transforms into a secure, multi-stage Azure DevOps pipeline. Read the **entire** agent file before making any changes.

---

## Modes

**Interactive mode** — ask the user what they want to change, confirm each modification, and explain any side-effects (e.g., adding `permissions.write` when a write-requiring safe output is introduced).

**Non-interactive mode** — apply the requested changes directly, validate the result, and report what was changed and why.

In both modes, follow the update workflow below.

---

## Update Workflow

### Step 1 — Read the Existing File

Read the full agent markdown file. Identify:

- **Front matter fields** currently set (name, description, engine, schedule, pool, target, workspace, etc.)
- **MCP servers** configured and their `allowed:` lists
- **Safe outputs** enabled and their configuration
- **Permissions** (`read` / `write` / both / neither)
- **Repositories and checkout** configuration
- **Steps** (setup, teardown, steps, post-steps)
- **Network** allow/blocked lists
- **Agent instructions** (the markdown body after the closing `---`)

Do not proceed until you understand the current state.

### Step 2 — Make Targeted Changes

Modify **only** what the user requests. Do not refactor unrelated sections, reorder fields, or rewrite the agent instructions unless asked.

When adding new fields, place them in the conventional front matter order:

```
name → description → target → engine → schedule → workspace → pool →
repos → tools → mcp-servers → safe-outputs →
triggers → steps → post-steps → setup → teardown → network →
permissions → parameters
```

Before changing the markdown body, re-examine the existing `steps:` /
`post-steps:` blocks and ask whether the requested behavior is a
hoist candidate:

- Is the work deterministic across runs (no agent reasoning needed)?
- Does it happen on every invocation (clone, cache restore, runtime install, artifact download)?
- Are the inputs fixed at compile time (repo URL, branch, tool versions)?
- → If yes to all three, hoist into `steps:` (pre-agent) or `post-steps:` (after-agent).

If the requested change is a hoist candidate, add it to the front
matter steps block rather than the prompt body. Draft the updated block,
call `validate_steps` with `allow_list: "full"`, and fix any returned
errors before writing the file.

> **`on.pr` knob update**: when changing `on.pr.branches` or
> `on.pr.paths`, also confirm whether `mode` (default `synthetic`) is
> appropriate. In `synthetic` mode the compiler emits a Setup-job ADO
> REST call to discover the open PR for `Build.SourceBranch` and
> leaves the top-level `trigger:` at the ADO default. Switch to
> `mode: policy` only if the operator has explicitly installed a
> Build Validation branch policy — that mode emits `trigger: none`
> and drops the synth wiring. Reference:
> [`docs/front-matter.md#pr-triggering-in-azure-repos`](../docs/front-matter.md#pr-triggering-in-azure-repos).

### Step 3 — Validate the Changes

Run through the validation checklist (see below) before finalizing. Fix any issues and inform the user of corrections made.

When you have local CLI access, two read-only commands give a quick
structural sanity check **before** you recompile or hand off to the
user:

```bash
# Compact summary of jobs, stages, steps, output decls, derived dependsOn
ado-aw inspect path/to/agent.md

# Resolved dependency graph (text by default; --format dot pipes to Graphviz)
ado-aw graph dump path/to/agent.md
```

These build the typed IR from the source and answer "did my change
add/remove the expected jobs?" and "did the output / dependency wiring
end up where I expected?" without writing any YAML to disk. The audit
docs in [`docs/audit.md`](../docs/audit.md) and the IR JSON contract
in [`docs/ir.md`](../docs/ir.md#public-json-summary-irsummary) cover
the underlying `PipelineSummary` schema if you want to script against
the JSON form.

### Step 4 — Recompile (if needed)

After any **front matter** changes, the pipeline YAML must be regenerated:

```bash
ado-aw compile <path/to/agent.md>
```

> **Important**: Changes to the **markdown body** (agent instructions) do **not** require recompilation — the agent reads its instructions from the source `.md` file at runtime.

---

## What Requires Recompilation

| Change | Recompile? |
|---|---|
| Any YAML front matter field | **Yes** — run `ado-aw compile` |
| Agent instructions (markdown body) | **No** — loaded at runtime |
| Adding/removing safe outputs | **Yes** |
| Changing schedule | **Yes** |
| Adding/removing MCP servers | **Yes** |
| Updating permissions | **Yes** |
| Editing the agent's task description | **No** |
| Adding steps / post-steps / setup / teardown | **Yes** |
| Changing network allow/blocked lists | **Yes** |

---

## Common Update Scenarios

### Adding a Safe Output Tool

Example: adding `comment-on-work-item` to an existing workflow.

1. Add the safe output configuration to the front matter:

```yaml
safe-outputs:
  # ... existing safe outputs ...
  comment-on-work-item:
    max: 3
    target: "MyProject\\MyTeam"   # Required — scoping policy
```

2. Optionally set `permissions.write` if you need cross-org writes or a named identity (not required — the executor defaults to `$(System.AccessToken)`):

```yaml
permissions:
  write: my-write-arm-connection
```

3. Update the agent instructions to explain when and how to use the new tool.

4. Recompile: `ado-aw compile <path/to/agent.md>`

### Changing the Schedule

Example: changing from daily to weekly.

```yaml
# Before
schedule: daily around 14:00

# After
schedule: weekly on monday around 9:00
```

The fuzzy schedule syntax scatters execution time deterministically based on the agent name hash. The actual cron time will be within ±60 minutes of the specified time.

**Schedule quick reference:**

| Expression | Meaning |
|---|---|
| `daily around 14:00` | Within ±60 min of 2 PM UTC |
| `weekly on monday` | Every Monday, scattered time |
| `weekly on friday around 17:00` | Friday ~5 PM UTC |
| `hourly` | Every hour, scattered minute |
| `every 2h` | Every 2 hours |
| `bi-weekly` | Every 14 days |
| `daily between 9:00 and 17:00` | Business hours |

Append `utc+N` or `utc-N` for timezone conversion: `daily around 9:00 utc-5`

To schedule on branches other than `main`, use the object form:

```yaml
schedule:
  run: weekly on monday around 9:00
  branches:
    - main
    - release/*
```

Recompile after any schedule change.

### Adding an MCP Server

Example: adding the first-class `azure-devops` tool.

```yaml
tools:
  azure-devops: true
  # Or with scoping:
  # azure-devops:
  #   toolsets: [repos, wit]
  #   allowed: [wit_get_work_item, core_list_projects]
  #   org: myorg
```

When adding `azure-devops`, also ensure:

- `permissions.read` is set (the agent needs a token to query ADO APIs)
- The compiler auto-adds ADO-specific hosts to the network allowlist

For custom containerized MCP servers:

```yaml
mcp-servers:
  my-tool:
    container: "node:20-slim"
    entrypoint: "node"
    entrypoint-args: ["path/to/server.js"]
    allowed:
      - do_thing
      - get_status
```

Custom MCPs **must** have an explicit `allowed:` list. Add any required external domains to `network.allowed`.

### Adding Permissions

Example: enabling read-only ADO API access for the agent.

```yaml
permissions:
  read: my-read-arm-connection
```

| Config | Effect |
|---|---|
| `read` only | Agent can query ADO APIs; no safe-output writes |
| `write` only | Agent has no ADO API access; safe-outputs can write |
| Both | Agent reads; safe-outputs write |
| Neither | No ADO tokens anywhere |

If adding write-requiring safe outputs (`create-pull-request`, `create-work-item`, `comment-on-work-item`, `update-work-item`, `create-wiki-page`, `update-wiki-page`, `link-work-items`, `upload-workitem-attachment`, `create-branch`, `create-git-tag`, `add-build-tag`, `add-pr-comment`, `reply-to-pr-comment`, `resolve-pr-thread`, `submit-pr-review`, `update-pr`, `queue-build`), you **must** also add `permissions.write`. The compiler will error otherwise.

### Adding Pre/Post Steps

When a behavior change is deterministic, happens on every run, and has
compile-time-fixed inputs, prefer `steps:` / `post-steps:` over adding
more instructions for the agent to execute with bash. Validate every
updated block with the author MCP server's `validate_steps` tool before
writing it:

```json
{
  "steps": [
    {
      "bash": "echo \"Preparing context...\"",
      "displayName": "Prepare context"
    }
  ],
  "allow_list": "full"
}
```

If the tool returns errors, fix the block and validate again.

**Inline steps** run inside the `Agent` job:

```yaml
steps:               # BEFORE agent runs
  - bash: echo "Preparing context..."
    displayName: "Prepare context"

post-steps:          # AFTER agent completes
  - bash: echo "Processing outputs..."
    displayName: "Post-process"
```

**Separate jobs** run before/after the entire pipeline:

```yaml
setup:               # Separate job BEFORE Agent
  - bash: echo "Provisioning..."
    displayName: "Setup"

teardown:            # Separate job AFTER SafeOutputs
  - bash: echo "Cleanup..."
    displayName: "Teardown"
```

### Updating Agent Instructions

Changes to the markdown body (after the closing `---`) do **not** require recompilation. The agent reads its instructions from the source `.md` file at runtime.

When updating instructions:

- Preserve the existing structure unless the user asks for a rewrite
- If new safe-output tools were added, explain when the agent should use them
- If new MCP servers were added, describe the new capabilities available
- Keep instructions concise — the agent reads them on every execution

---

## Validation Checklist

Before finalizing any update, verify:

1. **Write permissions**: Every write-requiring safe output has a corresponding `permissions.write` configured.

2. **MCP allow-lists**: Custom MCP servers (with `container:` or `url:`) have explicit `allowed:` lists.

3. **Schedule syntax**: The schedule expression uses valid fuzzy schedule syntax. Valid frequencies: `daily`, `weekly on <day>`, `hourly`, `every Nh`, `every N minutes`, `bi-weekly`, `tri-weekly`. Valid time specs: `around HH:MM`, `between HH:MM and HH:MM`.

4. **Repository aliases**: Every repo alias used in agent instructions or safe-output `repository:` fields exists as an entry in `repos:` with `checkout: true` (the default).

5. **Workspace consistency**: If `workspace: repo` is set, ensure `repos:` has at least one additional repository with `checkout: true`. If only `self` is checked out, `workspace: repo` is unnecessary (the compiler warns about this).

6. **Network domains**: If new MCPs or external services are added, ensure required domains are in `network.allowed`.

7. **Target compatibility**: Both `standalone` and `1es` targets support containerized MCPs via MCPG.

8. **Safe output `target` fields**: `comment-on-work-item` requires an explicit `target` field. `update-work-item` fields require explicit opt-in (`status: true`, `title: true`, etc.).

9. **Parameter names**: Runtime `parameters:` names must be valid ADO identifiers.

10. **Engine model**: If `engine` is set to the default (`claude-opus-4.7`), it can be omitted.

---

## Example: Before and After

### Before — simple scheduled agent

```markdown
---
name: "Code Review Bot"
description: "Reviews open PRs for common issues"
schedule: daily around 10:00
permissions:
  read: my-read-sc
---

## Instructions

Review all open pull requests and leave comments on any issues found.
```

### After — adding work item creation and weekly schedule

```markdown
---
name: "Code Review Bot"
description: "Reviews open PRs for common issues and creates tracking work items"
schedule: weekly on monday around 10:00
permissions:
  read: my-read-sc
  write: my-write-sc
safe-outputs:
  create-work-item:
    work-item-type: Bug
    tags:
      - automated
      - code-review
    max: 5
---

## Instructions

Review all open pull requests and leave comments on any issues found.

### When Issues Are Found

For each significant issue, create a work item using `create-work-item` with:
- A clear title describing the issue
- A description with the PR link, file path, and explanation

### When No Issues Are Found

Use `noop` with a summary of what was reviewed.
```

**Changes made:**
1. Updated `description` to reflect new capability
2. Changed `schedule` from `daily` to `weekly on monday`
3. Optionally added `permissions.write` (only needed for cross-org writes or named-identity attribution; the executor defaults to `$(System.AccessToken)`)
4. Added `safe-outputs.create-work-item` configuration
5. Updated agent instructions to describe when to create work items

**Recompilation required**: Yes — front matter was modified.

---

## Output Instructions

After completing an update:

1. **Summarize changes** — list each front matter field that was added, modified, or removed.
2. **Note recompilation** — state whether `ado-aw compile` is needed.
3. **Flag validation issues** — report any checklist items that were auto-corrected.
4. **Provide next steps**:

```
Next steps:
  1. Review the changes in <filename>.md
  2. Recompile: ado-aw compile <path/to/agent.md>
  3. Commit both the updated .md source and regenerated .lock.yml pipeline
```

If only agent instructions were changed:

```
Next steps:
  1. Review the changes in <filename>.md
  2. Commit the updated .md file (no recompilation needed)
```

---

## Reference

For complete field documentation, schema details, and all available safe output tools, see the full project reference:

<https://raw.githubusercontent.com/githubnext/ado-aw/main/AGENTS.md>
