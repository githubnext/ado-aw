# Copilot Instructions for Azure DevOps Agentic Pipelines

This repository contains a compiler for Azure DevOps pipelines that transforms
natural language markdown files with YAML front matter into Azure DevOps
pipeline definitions. The design is inspired by
[GitHub Agentic Workflows (gh-aw)](https://github.com/githubnext/gh-aw).

This page is the **high-level entry point** for the project. Each major concept
has its own complete reference under [`docs/`](docs/) — start here, then jump to
the relevant page when you need detail.

## Project Overview

### Purpose

The `ado-aw` compiler enables users to write pipeline definitions in a
human-friendly markdown format with YAML front matter, which gets compiled into
proper Azure DevOps YAML pipeline definitions. This approach:

- Makes pipeline authoring more accessible through natural language
- Enables AI agents to work safely in network-isolated sandboxes (via OneBranch)
- Provides a small, controlled set of tools for agents to complete work
- Validates outputs for correctness and conformity

Alongside the correctly generated pipeline yaml, an agent file is generated
from the remaining markdown and placed in `agents/` at the root of a consumer
repository. The pipeline yaml references the agent.

### Three-Stage Pipeline Model

Every compiled pipeline runs as three sequential jobs:

1. **Agent (Stage 1)** — runs the AI agent inside an AWF network-isolated
   sandbox with a read-only ADO token. The agent produces *safe-output
   proposals* (e.g. "create this PR", "comment on this work item") rather than
   acting directly.
2. **Detection (Stage 2)** — a separate agent inspects Stage 1's proposals for
   prompt injection, secret leaks, and other threats.
3. **SafeOutputs (Stage 3)** — a non-agent executor applies approved safe outputs
   using a write-capable ADO token that the agent never sees.

### Architecture

```
├── src/
│   ├── main.rs           # Entry point with clap CLI
│   ├── allowed_hosts.rs  # Core network allowlist definitions
│   ├── ecosystem_domains.rs # Ecosystem domain lookups (python, rust, node, etc.)
│   ├── engine.rs         # Engine enum, CLI params, model/version defaults
│   ├── compile/          # Pipeline compilation module
│   │   ├── mod.rs        # Module entry point and Compiler trait
│   │   ├── common.rs     # Shared helpers across targets
│   │   ├── standalone.rs # Standalone pipeline compiler
│   │   ├── onees.rs      # 1ES Pipeline Template compiler
│   │   ├── job.rs        # Job-level ADO template compiler (target: job)
│   │   ├── stage.rs      # Stage-level ADO template compiler (target: stage)
│   │   ├── gitattributes.rs # .gitattributes management for compiled pipelines
│   │   ├── filter_ir.rs  # Filter expression IR: Fact/Predicate types, lowering, validation, codegen
│   │   ├── pr_filters.rs # PR trigger filter generation (native ADO + gate steps)
│   │   ├── extensions/   # CompilerExtension trait and infrastructure extensions
│   │   │   ├── mod.rs    # Trait, Extension enum, collect_extensions(), re-exports
│   │   │   ├── github.rs # Always-on GitHub MCP extension
│   │   │   ├── safe_outputs.rs # Always-on SafeOutputs MCP extension
│   │   │   ├── trigger_filters.rs # Trigger filter extension (gate evaluator delivery)
│   │   │   └── tests.rs  # Extension integration tests
│   │   ├── codemods/     # Front-matter codemods (one file per transformation)
│   │   │   ├── mod.rs    # Codemod struct, CODEMODS registry, runner
│   │   │   ├── 0001_repos_unified.rs # Legacy repositories/checkout → repos codemod
│   │   │   ├── 0002_pool_object_form.rs # Legacy scalar pool → object form codemod
│   │   │   └── helpers.rs # take_key, insert_no_overwrite, rename_key, ConflictPolicy
│   │   ├── codemod_integration_test.rs # White-box rewrite-path tests (stub registry injection)
│   │   └── types.rs      # Front matter grammar and types
│   ├── init.rs           # Repository initialization for AI-first authoring
│   ├── execute.rs        # Stage 3 safe output execution
│   ├── fuzzy_schedule.rs # Fuzzy schedule parsing
│   ├── logging.rs        # File-based logging infrastructure
│   ├── mcp.rs            # SafeOutputs MCP server (stdio + HTTP)
│   ├── configure.rs      # `configure` CLI command — orchestration shim atop `src/ado/`
│   ├── enable.rs         # `enable` CLI command — registers ADO build definitions for compiled pipelines and ensures they are enabled
│   ├── disable.rs        # `disable` CLI command — sets queueStatus to disabled (default) or paused on matched definitions
│   ├── remove.rs         # `remove` CLI command — deletes matched ADO build definitions (with --yes / tty-prompt safety)
│   ├── list.rs           # `list` CLI command — renders matched ADO definitions with their latest-run state (text or JSON)
│   ├── ado/              # Shared Azure DevOps REST helpers (auth, list/match/PATCH/POST)
│   │   └── mod.rs        # Used by `configure` and the `enable` command (ADO REST helpers: auth, list/match/PATCH/POST)
│   ├── detect.rs         # Agentic pipeline detection (helper for `configure`)
│   ├── ndjson.rs         # NDJSON parsing utilities
│   ├── sanitize.rs       # Input sanitization for safe outputs
│   ├── validate.rs       # Structural input validators (char allowlists, format checks, injection detectors)
│   ├── agent_stats.rs    # OTel-based agent statistics parsing (token usage, duration, turns)
│   ├── hash.rs           # SHA-256 utilities for safe-output file integrity
│   ├── safeoutputs/      # Safe-output MCP tool implementations (Stage 1 → NDJSON → Stage 3)
│   │   ├── mod.rs
│   │   ├── add_build_tag.rs
│   │   ├── add_pr_comment.rs
│   │   ├── comment_on_work_item.rs
│   │   ├── create_branch.rs
│   │   ├── create_git_tag.rs
│   │   ├── create_issue.rs
│   │   ├── create_pull_request.rs
│   │   ├── create_wiki_page.rs
│   │   ├── create_work_item.rs
│   │   ├── link_work_items.rs
│   │   ├── missing_data.rs
│   │   ├── missing_tool.rs
│   │   ├── noop.rs
│   │   ├── queue_build.rs
│   │   ├── reply_to_pr_comment.rs
│   │   ├── report_incomplete.rs
│   │   ├── resolve_pr_thread.rs
│   │   ├── result.rs
│   │   ├── submit_pr_review.rs
│   │   ├── update_pr.rs
│   │   ├── update_wiki_page.rs
│   │   ├── update_work_item.rs
│   │   ├── upload_build_attachment.rs
│   │   ├── upload_pipeline_artifact.rs
│   │   └── upload_workitem_attachment.rs
│   ├── runtimes/         # Runtime environment implementations (one dir per runtime)
│   │   ├── mod.rs        # Module entry point
│   │   ├── lean/         # Lean 4 theorem prover runtime
│   │   │   ├── mod.rs    # Config types, install helpers
│   │   │   └── extension.rs # CompilerExtension impl
│   │   ├── python/       # Python runtime
│   │   │   ├── mod.rs    # Config types, install/auth helpers
│   │   │   └── extension.rs # CompilerExtension impl
│   │   ├── node/         # Node.js runtime
│   │   │   ├── mod.rs    # Config types, install/auth helpers
│   │   │   └── extension.rs # CompilerExtension impl
│   │   └── dotnet/       # .NET runtime
│   │       ├── mod.rs    # Config types, install/auth helpers
│   │       └── extension.rs # CompilerExtension impl
│   ├── data/
│   │   ├── base.yml          # Base pipeline template for standalone
│   │   ├── 1es-base.yml      # Base pipeline template for 1ES target
│   │   ├── job-base.yml      # Job-level ADO template for target: job
│   │   ├── stage-base.yml    # Stage-level ADO template for target: stage
│   │   ├── ecosystem_domains.json # Network allowlists per ecosystem
│   │   ├── init-agent.md     # Dispatcher agent template for `init` command
│   │   └── threat-analysis.md # Threat detection analysis prompt template
│   └── tools/            # First-class tool implementations (one dir per tool)
│       ├── mod.rs
│       ├── azure_devops/  # Azure DevOps MCP tool
│       │   ├── mod.rs
│       │   └── extension.rs # CompilerExtension impl
│       └── cache_memory/  # Persistent agent memory tool
│           ├── mod.rs
│           ├── extension.rs # CompilerExtension impl (compile-time)
│           └── execute.rs   # Stage 3 runtime (validate/copy)
├── ado-aw-derive/        # Proc-macro crate: #[derive(SanitizeConfig)], #[derive(SanitizeContent)]
├── examples/             # Example agent definitions
├── prompts/              # AI agent prompt files for workflow authoring tasks
│   ├── create-ado-agentic-workflow.md # Step-by-step guide for creating a new agentic pipeline
│   ├── update-ado-agentic-workflow.md # Guide for modifying an existing agentic pipeline
│   └── debug-ado-agentic-workflow.md  # Guide for troubleshooting a failing agentic pipeline
├── scripts/              # Supporting scripts shipped as release artifacts
│   ├── gate-eval.py      # Python gate evaluator (data-driven filter evaluation)
│   └── gate-spec.schema.json # JSON Schema for gate spec (generated from Rust types)
├── tests/                # Integration tests and fixtures
├── docs/                 # Per-concept reference documentation (see index below)
├── Cargo.toml            # Rust dependencies
└── README.md             # Project documentation
```

## Technology Stack

- **Language**: Rust (2024 edition) - Note: Rust 2024 edition exists and is the edition used by this project
- **CLI Framework**: clap v4 with derive macros
- **Error Handling**: anyhow for ergonomic error propagation
- **Async Runtime**: tokio with full features
- **YAML Parsing**: serde_yaml
- **MCP Server**: rmcp with server and transport-io features
- **Target Platform**: Azure DevOps Pipelines / OneBranch

## Documentation Index

The detailed reference for each concept lives in [`docs/`](docs/). Use this
index to jump to the right page.

### Prompt files for workflow authoring

- [`prompts/create-ado-agentic-workflow.md`](prompts/create-ado-agentic-workflow.md) — step-by-step
  guide for creating a new agentic pipeline from scratch (interactive and non-interactive modes).
- [`prompts/update-ado-agentic-workflow.md`](prompts/update-ado-agentic-workflow.md) — guide for
  modifying an existing agentic pipeline (read-then-update workflow with validation).
- [`prompts/debug-ado-agentic-workflow.md`](prompts/debug-ado-agentic-workflow.md) — guide for
  troubleshooting a failing agentic pipeline and filing a diagnostic report.

### Authoring agent files

- [`docs/front-matter.md`](docs/front-matter.md) — full agent file format
  (markdown body + YAML front matter grammar) with every supported field.
- [`docs/schedule-syntax.md`](docs/schedule-syntax.md) — fuzzy schedule time
  syntax (`daily around 14:00`, `weekly on monday`, timezones, scattering).
- [`docs/engine.md`](docs/engine.md) — `engine:` configuration (model,
  `timeout-minutes`, `version`, `agent`, `api-target`, `args`, `env`,
  `command`).
- [`docs/parameters.md`](docs/parameters.md) — ADO runtime parameters surfaced
  in the pipeline UI, including the auto-injected `clearMemory` parameter.
- [`docs/tools.md`](docs/tools.md) — `tools:` configuration (bash allow-list,
  `edit`, `cache-memory`, `azure-devops` MCP).
- [`docs/runtimes.md`](docs/runtimes.md) — `runtimes:` configuration (Lean 4,
  Python, Node.js, .NET).
- [`docs/targets.md`](docs/targets.md) — target platforms: `standalone`,
  `1es`, `job`, and `stage`.
- [`docs/safe-outputs.md`](docs/safe-outputs.md) — full reference for every
  safe-output tool agents can use to propose actions (PRs, work items, wiki
  pages, comments, etc.) plus their per-agent configuration.
- [`docs/ado-aw-debug.md`](docs/ado-aw-debug.md) — debug-only `ado-aw-debug:`
  front-matter section (`skip-integrity`, `create-issue` for filing GitHub
  issues from dogfood pipelines). NOT a regular safe-output.

### Compiler internals & operations

- [`docs/template-markers.md`](docs/template-markers.md) — every `{{ marker }}`
  in `src/data/base.yml`, `src/data/1es-base.yml`, `src/data/job-base.yml`, and `src/data/stage-base.yml` and how it is replaced.
- [`docs/cli.md`](docs/cli.md) — `ado-aw` CLI commands (`init`, `compile`,
  `check`, `mcp`, `mcp-http`, `execute`, `configure`).
- [`docs/mcp.md`](docs/mcp.md) — MCP server configuration (stdio containers,
  HTTP servers, env passthrough).
- [`docs/mcpg.md`](docs/mcpg.md) — MCP Gateway architecture and pipeline
  integration.
- [`docs/network.md`](docs/network.md) — AWF network isolation, default
  allowed domains, ecosystem identifiers, blocking, and ADO `permissions:`
  service-connection model.
- [`docs/extending.md`](docs/extending.md) — adding new CLI commands, compile
  targets, front-matter fields, template markers, safe-output tools,
  first-class tools, and runtimes; the `CompilerExtension` trait.
- [`docs/filter-ir.md`](docs/filter-ir.md) — filter expression IR
  specification: `Fact`/`Predicate` types, three-pass compilation (lower →
  validate → codegen), gate step generation, adding new filter types.
- [`docs/codemods.md`](docs/codemods.md) — front-matter codemod
  framework: detection-based transformations, automatic source
  rewrite on breaking-change updates, contributor workflow for
  adding codemods.
- [`docs/local-development.md`](docs/local-development.md) — local development
  setup notes.

## Development Guidelines

### Commit Message and PR Title Convention

This project uses [Conventional Commits](https://www.conventionalcommits.org/)
for automated releases via `release-please`. **PR titles are the commit
messages** — this repo uses squash-merge, so the PR title becomes the commit on
`main`.

All PR titles **must** follow the format:

```
type(optional scope): description
```

Common types: `feat`, `fix`, `chore`, `docs`, `refactor`, `test`, `ci`. PRs
with non-conforming titles will be blocked by CI and, if merged, will be
silently dropped from the changelog.

- **`feat`** — triggers a minor version bump and appears under "Features" in
  the changelog.
- **`fix`** — triggers a patch version bump and appears under "Bug Fixes".
- All other types (`chore`, `docs`, `refactor`, etc.) — no version bump, no
  changelog entry.

A PR titled `Allow workspace to target a repo alias` will be **ignored** by
release-please. The correct title is
`feat(compile): allow workspace to target a repo alias`.

### Rust Code Style

1. Use `anyhow::Result` for fallible functions
2. Leverage clap's derive macros for CLI argument parsing
3. Prefer explicit error messages with `anyhow::bail!` or `.context()`
4. Keep the binary fast—avoid unnecessary allocations and prefer streaming parsers

## Security Considerations

Following the gh-aw security model:

1. **Safe Outputs**: Only allow write operations through sanitized safe-output
   declarations — see [`docs/safe-outputs.md`](docs/safe-outputs.md).
2. **Network Isolation**: Pipelines run in OneBranch's network-isolated
   environment via AWF — see [`docs/network.md`](docs/network.md).
3. **Tool Allow-listing**: Agents have access to a limited, controlled set of
   tools — see [`docs/tools.md`](docs/tools.md) and
   [`docs/mcp.md`](docs/mcp.md).
4. **Input Sanitization**: Validate and sanitize all inputs before
   transformation — `src/validate.rs` and `src/sanitize.rs`.
5. **Permission Scoping**: Default to minimal permissions, require explicit
   elevation — see the *Permissions* section in
   [`docs/network.md`](docs/network.md).

## Testing

```bash
# Build the compiler
cargo build

# Run tests
cargo test

# Check for issues
cargo clippy
```

### Bash step lint

The `tests/bash_lint_tests.rs` integration test compiles a representative set
of fixtures and runs `shellcheck` against every literal `bash:` body in the
generated YAML. It catches silent-failure patterns that ADO's "fail on last
command" default would let through (e.g. `cd "$X"` without `|| exit`, tilde
inside double quotes, masked-return assignments).

The test is skipped if `shellcheck` is not on PATH. Install locally with
`brew install shellcheck` (macOS) or `apt-get install -y shellcheck` (Debian
/ Ubuntu); CI installs it in `.github/workflows/rust-tests.yml` and sets
`ENFORCE_BASH_LINT=1` so a missing shellcheck becomes a hard failure rather
than a silent skip.

When adding a new bash step, run `cargo test --test bash_lint_tests` and fix
anything it flags. If a finding is genuinely intentional, add a
`# shellcheck disable=SCxxxx` comment immediately above the offending line in
the bash body — shellcheck honours the directive and it's inert at runtime.

## Common Tasks

### Compile a markdown pipeline

```bash
cargo run -- compile ./path/to/agent.md
```

### Recompile all agentic pipelines in the current directory

```bash
# Auto-discovers and recompiles all detected agentic pipelines
cargo run -- compile
```

### Add a new dependency

```bash
cargo add <crate-name>
```

## File Naming Conventions

- Pipeline source files: `*.md` (markdown with YAML front matter)
- Compiled output: `*.yml` (Azure DevOps pipeline YAML)
- Rust source: `snake_case.rs`

## References

- [GitHub Agentic Workflows](https://github.com/githubnext/gh-aw) - Inspiration for this project
- [MCP Gateway (gh-aw-mcpg)](https://github.com/github/gh-aw-mcpg) - MCP routing gateway
- [AWF (gh-aw-firewall)](https://github.com/github/gh-aw-firewall) - Network isolation firewall
- [Azure DevOps YAML Schema](https://docs.microsoft.com/en-us/azure/devops/pipelines/yaml-schema)
- [OneBranch Documentation](https://aka.ms/onebranchdocs)
- [Clap Documentation](https://docs.rs/clap/latest/clap/)
- [Anyhow Documentation](https://docs.rs/anyhow/latest/anyhow/)
