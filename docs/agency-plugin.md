# Agency / Claude Code Plugin

_Part of the [ado-aw documentation](../AGENTS.md)._

ado-aw ships a [Claude Code plugin](https://code.claude.com/docs/en/plugins-reference)
that helps engineers author, compile, validate, operate, and debug Azure DevOps
agentic workflows from their agent (Claude Code or Copilot CLI). It is also an
[Agency](https://github.com/agency-microsoft/playground) marketplace plugin.

The plugin is mostly **packaging + wiring**, not new behavior: it surfaces
ado-aw's existing read-only [`mcp-author`](mcp-author.md) server and the
authoring playbooks in [`prompts/`](../prompts) as agent skills.

## Source of truth

The canonical, version-controlled plugin lives in this repository at
[`agency/plugins/ado-aw/`](../agency/plugins/ado-aw). It is the single source of
truth — `ado-aw init --agency` embeds these exact files via `include_str!` and
scaffolds them verbatim, so a consumer repo's plugin can never drift from the
canonical copy. A parity test (`tests/init_tests.rs`) asserts the two are
byte-for-byte identical.

## Layout

```
agency/plugins/ado-aw/
├── .claude-plugin/
│   └── plugin.json            # plugin manifest (name, version, mcpServers ref)
├── .mcp.json                  # wires the read-only `ado-aw mcp-author` server
├── agency.json                # Agency marketplace governance + external source
├── README.md                  # plugin landing page
├── agents/
│   └── ado-aw.md              # dispatcher subagent (routes to skills)
├── skills/
│   ├── create-workflow/SKILL.md
│   ├── update-workflow/SKILL.md
│   ├── debug-workflow/SKILL.md
│   ├── compile-and-validate/SKILL.md
│   ├── manage-lifecycle/SKILL.md
│   └── audit-build/SKILL.md
└── scripts/
    ├── doctor.sh              # prerequisite check (POSIX)
    └── doctor.ps1             # prerequisite check (Windows)
```

### Skills

| Skill | Source of truth | What it does |
| --- | --- | --- |
| `create-workflow` | `prompts/create-ado-agentic-workflow.md` | Author a new agentic workflow; ends with compile + lint. |
| `update-workflow` | `prompts/update-ado-agentic-workflow.md` | Read-then-update an existing workflow with validation. |
| `debug-workflow` | `prompts/debug-ado-agentic-workflow.md` | Diagnose a failing run via `trace_failure` + `audit_build`. |
| `compile-and-validate` | [`docs/cli.md`](cli.md), [`docs/ir.md`](ir.md) | `ado-aw compile` + `lint_workflow` + IR/graph inspection. |
| `manage-lifecycle` | [`docs/cli.md`](cli.md) | `enable`/`disable`/`remove`/`run`/`list`/`status`/`secrets`. |
| `audit-build` | [`docs/audit.md`](audit.md) | Download + analyze a finished build's artifacts. |

The `create`/`update`/`debug` skills load their authoritative playbook from a
**compiler-version-pinned** `raw.githubusercontent.com/.../v<version>/prompts/...`
URL, so the instructions always match the installed binary.

## MCP / tool surface

`.mcp.json` wires the local, read-only [`ado-aw mcp-author`](mcp-author.md) stdio
server, exposing `inspect_workflow`, `graph_summary`, `graph_dump`,
`step_dependencies`, `step_outputs`, `lint_workflow`, `whatif`, `catalog`,
`trace_failure`, and `audit_build`. Mutating actions (`compile`, `enable`,
`disable`, `remove`, `run`, `secrets`, `init`) are **not** in the MCP server — the
skills invoke them via the agent's shell, so the read-only firebreak is preserved.

## Self-contained marketplace

The repository root carries two generated marketplace catalogs that make the repo
directly installable:

| File | Engine |
| --- | --- |
| `.claude-plugin/marketplace.json` | Claude Code |
| `.github/plugin/marketplace.json` | GitHub Copilot |

Each lists the `ado-aw` plugin via `source: "./agency/plugins/ado-aw"`. After
running `ado-aw init --agency` (or pointing at this repo), register and install:

```text
/plugin marketplace add githubnext/ado-aw
/plugin install ado-aw@ado-aw
```

`ado-aw init --agency` writes the plugin under `agency/plugins/ado-aw/` **plus**
these root catalogs into the target repo, mirroring how the plugin is checked in
to ado-aw itself.

## Versioning

The plugin is **version-locked to the compiler**. `release-please` (via
`extra-files` in `release-please-config.json`) bumps, in lock-step on every
release:

- `plugin.json` `version` and both root catalogs' `metadata.version` +
  `plugins[0].version` (JSON updaters), and
- the version-pinned prompt URLs in `agents/ado-aw.md` and the
  create/update/debug skills (generic updater, keyed on the
  `<!-- x-release-please-version -->` marker).

## Identity & governance

`agency.json` declares `engines: ["claude", "copilot"]`, `category:
developer-tools`, and `governance: { layer: 4, certification: draft,
organization: github-next }`. Following the Agency convention, it carries **no
personal alias or email** — identity is the organization name only. It also
declares an external `source` pointer to this repo, used when the plugin is
listed in the shared Agency marketplace.

## Listing in the shared Agency marketplace

Hosting the plugin here makes it installable directly, but it is separate from
being _listed in_ `agency-microsoft/playground`. To list it, add a one-line
external-source entry to that repo's `marketplace-config.json`:

```json
"extPluginSources": {
  "https://github.com/githubnext/ado-aw.git": ["agency/plugins/ado-aw"]
}
```

The playground's sync bot then generates the shared marketplace entry from this
repo's `agency.json` `source` block.

## Related references

- [`docs/cli.md`](cli.md) — the `init --agency` flag and CLI counterparts for the
  skills' commands.
- [`docs/mcp-author.md`](mcp-author.md) — the read-only MCP server the plugin wires.
- [`prompts/`](../prompts) — the authoritative authoring playbooks the
  create/update/debug skills load.
