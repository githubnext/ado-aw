# ado-aw

**ado-aw** is a compiler that transforms human-friendly markdown files with YAML front matter into Azure DevOps pipeline definitions for running AI agents. It is inspired by [GitHub Agentic Workflows (gh-aw)](https://github.com/githubnext/gh-aw) and brings the same natural-language pipeline authoring experience to Azure DevOps.

## Overview

Writing Azure DevOps YAML pipelines by hand is complex and error-prone. **ado-aw** lets you describe an agent's task in plain markdown, then compiles that into a complete, validated Azure DevOps pipeline YAML.

```
┌──────────────────────────────────┐     ado-aw compile     ┌──────────────────────┐
│  agent.md                        │ ─────────────────────▶ │  pipeline.yml        │
│  (markdown + YAML front matter)  │                        │  (Azure DevOps YAML) │
└──────────────────────────────────┘                        └──────────────────────┘
```

Generated pipelines are production-ready and include:

- **Network isolation** via AWF (Agentic Workflow Firewall) — L7 domain allowlisting
- **MCP firewall** — tool-level filtering and audit logging for all agent tool calls
- **Safe outputs** — all write operations go through a sandboxed Stage 2 executor, not the agent directly
- **Deterministic scheduling** — fuzzy schedule syntax that automatically distributes execution times to avoid load spikes

## Installation

Pre-built binaries are available on the [Releases](https://github.com/githubnext/ado-aw/releases) page.

```bash
# Linux x64
curl -L https://github.com/githubnext/ado-aw/releases/latest/download/ado-aw-linux-x64 -o ado-aw
chmod +x ado-aw
```

To build from source (requires [Rust](https://rustup.rs)):

```bash
cargo build --release
```

## Quick Start

### 1. Create an agent definition

```bash
ado-aw create
```

This launches an interactive wizard that generates a `.md` file with the correct front matter. You then fill in the agent instructions in the markdown body.

Alternatively, write one by hand:

```markdown
---
name: "Daily Code Review"
description: "Reviews recent commits and posts a summary"
schedule: daily around 9:00
mcp-servers:
  ado: true
safe-outputs:
  create-work-item:
    work-item-type: Task
---

## Your Task

Review the most recent commits in this repository and create a work item summarizing any quality issues you find.
```

### 2. Compile to Azure DevOps YAML

```bash
ado-aw compile my-agent.md
# Writes my-agent.yml
```

### 3. Check pipeline is up to date

```bash
ado-aw check my-agent.md my-agent.yml
```

Use this in CI to fail the build when the YAML is out of sync with its source markdown.

## Input Format

Agent definitions are markdown files with a YAML front matter block (delimited by `---`). Key fields:

| Field | Description | Default |
|-------|-------------|---------|
| `name` | Human-readable agent name | *(required)* |
| `description` | One-line description | *(required)* |
| `target` | Output target: `standalone` or `1es` | `standalone` |
| `engine` | AI model (e.g. `claude-opus-4.5`, `gpt-5.2-codex`) | `claude-opus-4.5` |
| `schedule` | When to run — fuzzy or cron-style | *(none)* |
| `pool` | Azure DevOps agent pool name | `AZS-1ES-L-MMS-ubuntu-22.04` |
| `mcp-servers` | MCP servers the agent may use | *(none)* |
| `safe-outputs` | Write-back tool configuration | *(none)* |
| `permissions` | ARM service connections for ADO token acquisition | *(none)* |
| `network` | Additional allowed / blocked host patterns | *(none)* |

See [AGENTS.md](AGENTS.md) for the complete field reference and examples.

### Fuzzy Schedule Syntax

Instead of writing raw cron expressions, use plain English:

```yaml
schedule: daily around 14:00          # Within ±60 min of 2 PM UTC
schedule: weekly on monday            # Monday, scattered time
schedule: every 6h                    # Every 6 hours
schedule: daily around 3pm utc+9      # Timezone aware (2 PM JST → 5 AM UTC)
```

The compiler uses a deterministic hash of the agent name to scatter execution times, so different agents naturally spread their load without explicit coordination.

### MCP Servers

Built-in servers are enabled with `true` or configured with an allow-list. Custom servers supply a `command:`:

```yaml
mcp-servers:
  ado: true                # All ADO tools
  icm:
    allowed:               # Restrict to specific tools
      - create_incident
      - get_incident
  my-tool:                 # Custom MCP server
    command: "node"
    args: ["path/to/server.js"]
    allowed:
      - process_data
```

### Safe Outputs

Agents cannot write directly to external systems. They declare *intent* via safe outputs, which are validated and executed by an isolated Stage 2 job:

```yaml
safe-outputs:
  create-pull-request:
    target-branch: main
    auto-complete: true
  create-work-item:
    work-item-type: Task
    assignee: "user@example.com"
```

## CLI Reference

```
ado-aw <COMMAND>

Commands:
  create       Interactively create a new agent markdown file
  compile      Compile a markdown file to Azure DevOps pipeline YAML
  check        Verify a compiled pipeline matches its source markdown
  mcp          Run as an MCP server (safe outputs)
  execute      Execute safe outputs from Stage 1 (Stage 2 of the pipeline)
  proxy        Start an HTTP proxy for network filtering
  mcp-firewall Start an MCP firewall server that filters tool calls

Options:
  -v, --verbose  Enable info-level logging
  -d, --debug    Enable debug-level logging
  -h, --help     Print help
  -V, --version  Print version
```

## Target Platforms

| Target | Description |
|--------|-------------|
| `standalone` *(default)* | Self-contained 3-job pipeline with AWF network isolation |
| `1es` | Extends the 1ES Unofficial Pipeline Template (`agencyJob`) |

## Architecture

```
src/
├── main.rs              # CLI entry point (clap)
├── compile/             # Pipeline compilation
│   ├── standalone.rs    # Standalone target
│   ├── onees.rs         # 1ES target
│   └── types.rs         # Front matter grammar
├── execute.rs           # Stage 2 safe output executor
├── mcp.rs               # Safe outputs MCP server
├── mcp_firewall.rs      # MCP firewall server
├── proxy.rs             # Network proxy
├── fuzzy_schedule.rs    # Fuzzy schedule parser
└── tools/               # Safe output tool implementations
```

## Development

```bash
# Build
cargo build

# Run tests
cargo test

# Lint
cargo clippy
```

Integration tests live in `tests/` and use golden-file fixtures under `tests/fixtures/`.

## Related Projects

- **[gh-aw](https://github.com/githubnext/gh-aw)** — GitHub Agentic Workflows, the project that inspired ado-aw. It provides the same natural-language workflow authoring experience for GitHub Actions.

## License

[MIT](LICENSE)