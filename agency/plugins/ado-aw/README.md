# ado-aw — Azure DevOps Agentic Workflows

Author, compile, validate, operate, and debug **ado-aw** agentic workflows for
Azure DevOps from your agent (Claude Code or Copilot CLI). Each agentic workflow
compiles to a secure, multi-stage Azure DevOps pipeline.

[ado-aw](https://github.com/githubnext/ado-aw) compiles human-friendly Markdown
agent specs (Markdown body + YAML front matter) into secure, multi-stage Azure
DevOps pipelines that run AI agents in network-isolated sandboxes. Every compiled
pipeline runs three sequential jobs:

1. **Agent** — runs the AI agent in an AWF network-isolated sandbox with a
   read-only ADO token, producing *safe-output proposals* (not direct actions).
2. **Detection** — a separate agent inspects the proposals for prompt injection,
   secret leaks, and other threats.
3. **SafeOutputs** — a non-agent executor applies approved safe outputs with a
   write-capable token the agent never sees.

## What this plugin provides

- A dispatcher **agent** (`agents/ado-aw.md`) that routes authoring/ops requests.
- A read-only **MCP server** wired via `.mcp.json` to `ado-aw mcp-author`
  (inspect / graph / lint / whatif / trace / audit / catalog).
- Six **skills**:
  - `create-workflow` — author a new pipeline from scratch (compile + lint).
  - `update-workflow` — read-then-update an existing pipeline with validation.
  - `debug-workflow` — diagnose a failing pipeline (`trace_failure`, `audit_build`).
  - `compile-and-validate` — `ado-aw compile` + `lint_workflow` + IR/graph inspection.
  - `manage-lifecycle` — enable / disable / remove / run / list / status / secrets.
  - `audit-build` — download and analyze a finished build's artifacts.

## Installing

This repository is itself an installable plugin marketplace — the root
`.claude-plugin/marketplace.json` (Claude) and `.github/plugin/marketplace.json`
(Copilot) catalogs register the `ado-aw` plugin (whose files live under
`agency/plugins/ado-aw/`). From your agent:

```text
/plugin marketplace add githubnext/ado-aw
/plugin install ado-aw@ado-aw
```

`ado-aw init --agency` scaffolds the same catalogs + plugin into a consumer repo,
so that repo becomes registerable the same way.

## Prerequisites

The `ado-aw` compiler must be on `PATH`. Run the bundled doctor check first:

```bash
bash ./scripts/doctor.sh    # macOS / Linux
```
```powershell
.\scripts\doctor.ps1        # Windows
```

If `ado-aw` is missing, install it from the
[releases page](https://github.com/githubnext/ado-aw/releases/latest). ADO-facing
skills (`debug-workflow`, `audit-build`, `manage-lifecycle`) additionally rely on
`gh`/`az` auth or an ADO PAT, resolved through ado-aw's normal auth path.

## Security model

This plugin adds **no new write paths**. It only adds a read-only MCP server plus
`ado-aw` CLI invocations the user could already run. All mutation stays inside
ado-aw's three-stage safe-output model. No remote MCP endpoints are introduced
and no secrets are stored in the plugin tree.

## Source of truth & versioning

The canonical plugin lives in the ado-aw repository at
[`agency/plugins/ado-aw`](https://github.com/githubnext/ado-aw/tree/main/agency/plugins/ado-aw)
and is **version-locked to the compiler**: release automation bumps the plugin
version and the pinned prompt URLs in lock-step with each ado-aw release. The
Agency marketplace lists it via an external `source` pointer (`agency.json`).

## License

MIT — see the [ado-aw repository](https://github.com/githubnext/ado-aw).
