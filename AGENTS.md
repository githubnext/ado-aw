# Copilot Instructions for Azure DevOps Agentic Workflows

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

**Optional manual review.** When a safe output is configured with
`require-approval` (see [`docs/safe-outputs.md`](docs/safe-outputs.md)), an
agentless `ManualReview` job (`pool: server`, `ManualValidation@1`) is inserted
between Detection and SafeOutputs to pause for human approval. With a mix of
gated and non-gated outputs, Stage 3 splits into an automatic `SafeOutputs` job
(applies non-gated outputs immediately) and a `SafeOutputs_Reviewed` job (gated
behind `ManualReview`, publishes `safe_outputs_reviewed`). The gate is
fail-closed and only pauses when the agent actually proposed a reviewed output.

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
│   │   ├── agentic_pipeline.rs # Canonical Setup → Agent → Detection → (ManualReview?) → SafeOutputs(+SafeOutputs_Reviewed?) → Teardown → Conclusion shape (Conclusion emitted when configured; shared by every target); BuiltPipelineContext, build_pipeline_context, build_canonical_jobs, per-job builders incl. build_manual_review_job + SafeOutputsVariant split, fold_agent_conditions, agent_job_variables_hoist
│   │   ├── standalone.rs # Standalone pipeline compiler
│   │   ├── standalone_ir.rs # Standalone target typed-IR builder
│   │   ├── onees.rs      # 1ES Pipeline Template compiler
│   │   ├── onees_ir.rs   # 1ES target typed-IR builder
│   │   ├── job.rs        # Job-level ADO template compiler (target: job)
│   │   ├── job_ir.rs     # Job target typed-IR builder
│   │   ├── stage.rs      # Stage-level ADO template compiler (target: stage)
│   │   ├── stage_ir.rs   # Stage target typed-IR builder
│   │   ├── source_path_guard.rs # Validation guard for untrusted workflow source-path inputs used by audit + mcp_author
│   │   ├── gitattributes.rs # .gitattributes management for compiled pipelines
│   │   ├── filter_ir.rs  # Filter expression IR: Fact/Predicate types, lowering, validation, codegen
│   │   ├── pr_filters.rs # PR trigger filter generation (native ADO + gate steps)
│   │   ├── extensions/   # CompilerExtension trait and infrastructure extensions
│   │   │   ├── mod.rs    # Trait, Extension enum, collect_extensions(), re-exports
│   │   │   ├── ado_aw_marker.rs # Always-on metadata marker extension (emits # ado-aw-metadata JSON)
│   │   │   ├── github.rs # Always-on GitHub MCP extension
│   │   │   ├── safe_outputs.rs # Always-on SafeOutputs MCP extension
│   │   │   ├── ado_script.rs # Always-on ado-script extension (gate evaluator + runtime-import resolver + execution-context precomputes, per-job downloads)
│   │   │   ├── exec_context/ # Always-on execution-context extension (issue #860)
│   │   │   │   ├── mod.rs    # ExecContextExtension; CompilerExtension impl; contributor fan-out
│   │   │   │   ├── contributor.rs # Internal ContextContributor trait + Contributor enum
│   │   │   │   ├── ci_push.rs # CiPushContextContributor — push-build context facts for CI runs
│   │   │   │   ├── manual.rs # ManualContextContributor — manually queued build context facts
│   │   │   │   ├── pipeline.rs # PipelineContextContributor — shared pipeline/run metadata facts
│   │   │   │   ├── pr.rs     # PrContextContributor — stages aw-context/pr/* for PR builds
│   │   │   │   ├── pr_checks.rs # PrChecksContextContributor — PR validation / policy-check facts
│   │   │   │   ├── repo.rs   # RepoContextContributor — repository identity / remote facts
│   │   │   │   ├── schedule.rs # ScheduleContextContributor — scheduled-run context facts
│   │   │   │   └── workitem.rs # WorkItemContextContributor — linked work-item context facts
│   │   │   ├── azure_cli.rs # Always-on Azure CLI extension (runtime detection, AWF mounts, az allowlist)
│   │   │   └── tests.rs  # Extension integration tests
│   │   ├── codemods/     # Front-matter codemods (one file per transformation)
│   │   │   ├── mod.rs    # Codemod struct, CODEMODS registry, runner
│   │   │   ├── 0001_repos_unified.rs # Legacy repositories/checkout → repos codemod
│   │   │   ├── 0002_pool_object_form.rs # Legacy scalar pool → object form codemod
│   │   │   ├── 0003_flatten_work_item_config.rs # Nested work-item: block → flat fields (title-prefix, area-path, etc.) codemod
│   │   │   └── helpers.rs # take_key, insert_no_overwrite, rename_key, ConflictPolicy
│   │   ├── codemod_integration_test.rs # White-box rewrite-path tests (stub registry injection)
│   │   ├── types.rs      # Front matter grammar and types
│   │   └── ir/           # Typed Azure DevOps pipeline IR (see docs/ir.md)
│   │       ├── mod.rs    # Pipeline / PipelineBody / PipelineShape root types
│   │       ├── ids.rs    # Typed StageId / JobId / StepId newtypes
│   │       ├── step.rs   # Step variants (Bash, Task, Checkout, Download, Publish, RawYaml)
│   │       ├── tasks/    # Typed builder structs for built-in ADO tasks (one file per task; new()+typed setters+into_step(); command-enum dispatch for Docker/DotNet/NuGet/Npm/UniversalPackages; typestate builders for PowerShell; docker.rs canonical template)
│   │       ├── job.rs    # Job, Pool, TemplateContext, JobVariable
│   │       ├── stage.rs  # Stage + external-params wrap
│   │       ├── env.rs    # Typed EnvValue (Literal, AdoMacro, PipelineVar, Secret, StepOutput, Coalesce, Concat)
│   │       ├── condition.rs # Typed Condition / Expr AST + codegen to ADO condition syntax
│   │       ├── output.rs # OutputDecl / OutputRef + location-aware lowering
│   │       ├── graph.rs  # Dependency graph: validation, edge derivation, isOutput promotion, cycle detection
│   │       ├── lower.rs  # IR → serde_yaml::Value lowering
│   │       ├── emit.rs   # Thin `lower() + serde_yaml::to_string()` wrapper
│   │       └── summary.rs # Public, serializable PipelineSummary / GraphSummary for agent-facing tooling (see docs/ir.md Public JSON summary)
│   ├── init.rs           # Repository initialization for AI-first authoring (incl. `--agency` plugin scaffold, embeds agency/plugins/ado-aw/ via include_str!)
│   ├── execute.rs        # Stage 3 safe output execution
│   ├── fuzzy_schedule.rs # Fuzzy schedule parsing
│   ├── logging.rs        # File-based logging infrastructure
│   ├── mcp.rs            # SafeOutputs MCP server (stdio + HTTP)
│   ├── mcp_author/       # Author-facing read-only MCP server for local IDE/Copilot Chat integrations
│   │   ├── mod.rs        # Tool router + handlers for inspect/graph/whatif/lint/catalog/trace/audit
│   │   └── tests.rs      # MCP-author integration / contract tests
│   ├── configure.rs      # `configure` CLI command (deprecated) — hidden alias forwarding to `secrets set GITHUB_TOKEN`
│   ├── secrets.rs        # `secrets set/list/delete` subcommand group — manages pipeline variables (never prints values from `list`)
│   ├── enable.rs         # `enable` CLI command — registers ADO build definitions for compiled pipelines and ensures they are enabled
│   ├── disable.rs        # `disable` CLI command — sets queueStatus to disabled (default) or paused on matched definitions
│   ├── remove.rs         # `remove` CLI command — deletes matched ADO build definitions (with --yes / tty-prompt safety)
│   ├── list.rs           # `list` CLI command — renders matched ADO definitions with their latest-run state (text or JSON)
│   ├── status.rs         # `status` CLI command — denser per-pipeline status block (thin renderer over `list`'s data path)
│   ├── run.rs            # `run` CLI command — queues builds for matched definitions, optional polling to completion (module entry is `dispatch`)
│   ├── ado/              # Shared Azure DevOps REST helpers (auth, list/match/PATCH/POST)
│   │   ├── mod.rs        # Shared ADO REST helpers used by all lifecycle commands (`enable`, `disable`, `list`, `status`, `run`, `remove`, `secrets`)
│   │   └── discovery.rs  # Project-scope pipeline discovery (`--all-repos` / `--source` flags)
│   ├── audit/            # `ado-aw audit` command — downloads pipeline artifacts and runs analyzers
│   │   ├── mod.rs        # Shared audit data types; AuditData report model
│   │   ├── cli.rs        # CLI entry point for the `audit` subcommand
│   │   ├── model.rs      # AuditData and supporting report structs
│   │   ├── findings.rs   # Finding severity levels and structured finding types
│   │   ├── cache.rs      # Artifact download cache (keyed on build-id)
│   │   ├── pipeline_graph.rs # IR/runtime graph correlation that populates AuditData.pipeline_graph
│   │   ├── url.rs        # Build-reference parsing (bare ID, full ADO URL)
│   │   ├── analyzers/    # Per-signal analyzers that populate AuditData sections
│   │   │   ├── mod.rs
│   │   │   ├── detection.rs    # Detection-stage artifact analysis
│   │   │   ├── firewall.rs     # AWF network log analysis
│   │   │   ├── jobs.rs         # Build timeline / job-level analysis
│   │   │   ├── mcp.rs          # MCP tool-call analysis
│   │   │   ├── missing.rs      # Missing-tool / missing-data / noop safe-output analysis
│   │   │   ├── otel.rs         # OTel agent stats (token usage, duration, turns)
│   │   │   ├── policy.rs       # Policy-level findings (safe-output integrity, prompt injection signals)
│   │   │   └── safe_outputs.rs # Safe-output NDJSON analysis
│   │   └── render/       # Report renderers
│   │       ├── mod.rs
│   │       ├── console.rs # Human-readable console report
│   │       └── json.rs    # Machine-readable AuditData JSON
│   ├── inspect/          # `ado-aw inspect` / `graph` / `trace` / `whatif` / `lint` / `catalog` — read-only IR queries
│   │   ├── mod.rs        # Module entry; public re-exports of every dispatcher
│   │   ├── cli.rs        # Dispatchers (`dispatch_inspect`, `dispatch_graph`, …) and option structs
│   │   ├── graph_query.rs # Text/DOT renderers for the resolved dependency graph
│   │   ├── graph_deps.rs # `ado-aw graph deps`: upstream/downstream dependency traversal
│   │   ├── graph_outputs.rs # `ado-aw graph outputs`: producer/consumer output-reference table
│   │   ├── trace.rs      # `ado-aw trace`: correlate audit telemetry with the local IR graph
│   │   ├── whatif.rs     # `ado-aw whatif`: static downstream skip classification for failures
│   │   ├── lint.rs       # `ado-aw lint`: structural workflow lint checks
│   │   └── catalog.rs    # `ado-aw catalog`: list in-tree registries (tools, runtimes, models, etc.)
│   ├── detect.rs         # Agentic workflow detection — discovers compiled pipelines; used by all lifecycle commands
│   ├── update_check.rs   # Version update check — queries GitHub Releases and prints advisory when newer version is available
│   ├── ndjson.rs         # NDJSON parsing utilities
│   ├── sanitize.rs       # Input sanitization for safe outputs
│   ├── secure.rs         # Validated newtype value objects (parse-don't-validate path/identifier types)
│   ├── validate.rs       # Structural input validators (char allowlists, format checks, injection detectors)
│   ├── agent_stats.rs    # OTel-based agent statistics parsing (token usage, duration, turns)
│   ├── hash.rs           # SHA-256 utilities for safe-output file integrity
│   ├── safe_outputs/     # Safe-output MCP tool implementations (Stage 1 → NDJSON → Stage 3)
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
├── .claude-plugin/       # Root Claude marketplace catalog (makes the repo installable via `/plugin marketplace add`); release-please-versioned
│   └── marketplace.json  # Lists the ado-aw plugin with source ./agency/plugins/ado-aw
├── .github/plugin/       # Copilot marketplace catalog (mirrors .claude-plugin/marketplace.json for Copilot); release-please-versioned
│   └── marketplace.json  # Lists the ado-aw plugin with source ./agency/plugins/ado-aw
├── agency/               # Agency / Claude Code marketplace plugin (canonical source of truth)
│   └── plugins/ado-aw/   # Version-locked plugin (release-please bumps version + pinned prompt URLs); listed in Agency marketplace via external `source`; scaffolded into consumer repos by `ado-aw init --agency`
│       ├── .claude-plugin/ # plugin.json (manifest)
│       ├── .mcp.json     # Wires read-only `ado-aw mcp-author` stdio server
│       ├── README.md     # Plugin readme
│       ├── agency.json   # Marketplace governance metadata + external source pointer
│       ├── agents/ado-aw.md # Dispatcher subagent
│       ├── skills/       # 6 SKILL.md playbooks (create/update/debug-workflow, compile-and-validate, manage-lifecycle, audit-build)
│       └── scripts/      # doctor.{sh,ps1} prerequisite checks
├── examples/             # Example agent definitions
├── prompts/              # AI agent prompt files for workflow authoring tasks
│   ├── create-ado-agentic-workflow.md # Step-by-step guide for creating a new agentic workflow
│   ├── update-ado-agentic-workflow.md # Guide for modifying an existing agentic workflow
│   └── debug-ado-agentic-workflow.md  # Guide for troubleshooting a failing agentic workflow
├── scripts/              # Supporting scripts shipped as release artifacts
│   └── ado-script/       # TypeScript workspace for bundled gate/import helpers plus execution-context, conclusion, and approval-summary bundles
│       └── src/
│           ├── gate/     # Gate evaluator source (bundled to gate.js)
│           ├── import/   # Runtime prompt resolver source (bundled to import.js)
│           ├── exec-context-pr/ # PR-context precompute source (bundled to exec-context-pr.js)
│           ├── exec-context-pr-synth/ # Synthetic-PR resolver source (bundled to exec-context-pr-synth.js)
│           ├── exec-context-manual/ # Manual-run context source (bundled to exec-context-manual.js)
│           ├── exec-context-pipeline/ # Pipeline-completion context source (bundled to exec-context-pipeline.js)
│           ├── exec-context-ci-push/ # CI/push context source (bundled to exec-context-ci-push.js)
│           ├── exec-context-workitem/ # Linked work-item context source (bundled to exec-context-workitem.js)
│           ├── exec-context-schedule/ # Scheduled-run context source (bundled to exec-context-schedule.js)
│           ├── exec-context-pr-checks/ # PR validation checks context source (bundled to exec-context-pr-checks.js)
│           ├── exec-context-repo/ # Repository identity context source (bundled to exec-context-repo.js)
│           ├── conclusion/ # Conclusion-job reporter source (bundled to conclusion.js)
│           ├── approval-summary/ # Safe-outputs summary renderer (bundled to approval-summary.js; end-of-Agent-job summary tab)
│           └── shared/   # Shared modules across bundles (auth, ado-client, env-facts, types.gen.ts)
├── tests/                # Integration tests and fixtures
├── docs/                 # Per-concept reference documentation (see index below)
├── Cargo.toml            # Rust dependencies
└── README.md             # Project documentation
```

## Technology Stack

- **Language**: Rust (2024 edition) - Note: Rust 2024 edition exists and is the edition used by this project
- **CLI Framework**: clap v4 with derive macros
- **Error Handling**: anyhow for ergonomic error propagation
- **Bundled scripts**: TypeScript + ncc (`scripts/ado-script/`) — compiled gate evaluator, runtime import resolver, and execution-context precompute helpers; see [`docs/ado-script.md`](docs/ado-script.md).
- **Async Runtime**: tokio with full features
- **YAML Parsing**: serde_yaml
- **MCP Server**: rmcp with server and transport-io features
- **Target Platform**: Azure DevOps Pipelines / OneBranch

## Documentation Index

The detailed reference for each concept lives in [`docs/`](docs/). Use this
index to jump to the right page.

### Prompt files for workflow authoring

- [`prompts/create-ado-agentic-workflow.md`](prompts/create-ado-agentic-workflow.md) — step-by-step
  guide for creating a new agentic workflow from scratch (interactive and non-interactive modes).
- [`prompts/update-ado-agentic-workflow.md`](prompts/update-ado-agentic-workflow.md) — guide for
  modifying an existing agentic workflow (read-then-update workflow with validation).
- [`prompts/debug-ado-agentic-workflow.md`](prompts/debug-ado-agentic-workflow.md) — guide for
  troubleshooting a failing agentic workflow and filing a diagnostic report.

### Authoring agent files

- [`docs/front-matter.md`](docs/front-matter.md) — full agent file format
  (markdown body + YAML front matter grammar) with every supported field.
- [`docs/runtime-imports.md`](docs/runtime-imports.md) — runtime prompt import
  markers, path resolution, and `inlined-imports:` behavior.
- [`docs/schedule-syntax.md`](docs/schedule-syntax.md) — fuzzy schedule time
  syntax (`daily around 14:00`, `weekly on monday`, timezones, scattering).
- [`docs/engine.md`](docs/engine.md) — `engine:` configuration (model,
  `timeout-minutes`, `version`, `agent`, `api-target`, `args`, `env`,
  `command`).
- [`docs/parameters.md`](docs/parameters.md) — ADO runtime parameters surfaced
  in the pipeline UI, including the auto-injected `clearMemory` parameter.
- [`docs/conclusion.md`](docs/conclusion.md) — `conclusion:` configuration for
  the always-running post-pipeline housekeeping job that files work-item
  reports for failures and diagnostic signals.
- [`docs/tools.md`](docs/tools.md) — `tools:` configuration (bash allow-list,
  `edit`, `cache-memory`, `azure-devops` MCP).
- [`docs/runtimes.md`](docs/runtimes.md) — `runtimes:` configuration (Lean 4,
  Python, Node.js, .NET).
- [`docs/targets.md`](docs/targets.md) — target platforms: `standalone`,
  `1es`, `job`, and `stage`.
- [`docs/execution-context.md`](docs/execution-context.md) — built-in
  `aw-context/` precompute contributors for PR, manual, pipeline,
  CI/push, work-item, scheduled, PR-check, and repository context;
  configured via the `execution-context:` front-matter block.
- [`docs/safe-outputs.md`](docs/safe-outputs.md) — full reference for every
  safe-output tool agents can use to propose actions (PRs, work items, wiki
  pages, comments, etc.) plus their per-agent configuration.
- [`docs/safe-output-permissions.md`](docs/safe-output-permissions.md) —
  diagnosis and fix reference for Stage 3 401/403 failures: the
  default build identity (PCBS vs project-scoped Build Service),
  `$(System.AccessToken)` semantics, the "Limit job authorization
  scope to current project" toggle, permission-bitmask decoder,
  REST recipe for inspecting ACEs, and the three fix paths.
- [`docs/ado-aw-debug.md`](docs/ado-aw-debug.md) — debug-only `ado-aw-debug:`
  front-matter section (`skip-integrity`, `create-issue` for filing GitHub
  issues from dogfood pipelines). NOT a regular safe-output.
- [`docs/supply-chain.md`](docs/supply-chain.md) — optional `supply-chain:`
  front-matter section that mirrors the compiler, AWF binary, ado-script
  bundle, and AWF/MCPG images from an internal Azure DevOps Artifacts feed
  and/or container registry (NuGet `DownloadPackage@1` + ACR `az acr login`),
  with asymmetric auth (feed defaults to `$(System.AccessToken)`; registry
  requires a service connection).

### Compiler internals & operations

- [`docs/ir.md`](docs/ir.md) — typed Azure DevOps pipeline IR (`Pipeline`, jobs/stages/steps, output refs, graph pass, lowering, target builders, and the public JSON summary consumed by agent-facing tooling).
- [`docs/cli.md`](docs/cli.md) — `ado-aw` CLI commands (`init`, `compile`,
  `check`, `mcp`, `mcp-http`, `execute`, `secrets`, `enable`, `disable`,
  `remove`, `list`, `status`, `run`, `audit`, `mcp-author`, `trace`,
  `inspect`, `graph`, `whatif`, `lint`, `catalog`; `configure` is a
  deprecated hidden alias and `export-gate-schema` is a hidden build-time tool).
- [`docs/agency-plugin.md`](docs/agency-plugin.md) — the Agency / Claude Code
  plugin (`agency/plugins/ado-aw/`): canonical layout, six skills, `mcp-author`
  wiring, the self-contained root marketplace catalogs, `init --agency`
  scaffolding, release-please version-locking, and shared-marketplace listing.
- [`docs/audit.md`](docs/audit.md) — `ado-aw audit`: accepted build-id / URL
  forms, artifact layout, cache behavior, rejection tracing, and `AuditData`
  report shape.
- [`docs/mcp.md`](docs/mcp.md) — MCP server configuration (stdio containers,
  HTTP servers, env passthrough).
- [`docs/mcp-author.md`](docs/mcp-author.md) — author-facing MCP server
  (stdio); exposes `inspect`, `graph`, `whatif`, `lint`, `catalog`, `trace`,
  `audit_build` over MCP for IDE/Copilot Chat agents.
- [`docs/mcpg.md`](docs/mcpg.md) — MCP Gateway architecture and pipeline
  integration.
- [`docs/network.md`](docs/network.md) — AWF network isolation, default
  allowed domains, ecosystem identifiers, blocking, and ADO `permissions:`
  service-connection model.
- [`docs/extending.md`](docs/extending.md) — adding new CLI commands, compile
  targets, front-matter fields, typed IR extensions, safe-output tools,
  first-class tools, and runtimes; the `CompilerExtension` trait.
- [`docs/filter-ir.md`](docs/filter-ir.md) — filter expression IR
  specification: `Fact`/`Predicate` types, three-pass compilation (lower →
  validate → codegen), gate step generation, adding new filter types.
- [`docs/codemods.md`](docs/codemods.md) — front-matter codemod
  framework: detection-based transformations, automatic source
  rewrite on breaking-change updates, contributor workflow for
  adding codemods.
- [`docs/ado-script.md`](docs/ado-script.md) — `ado-script` workspace
  (`scripts/ado-script/`): the bundled TypeScript runtime helpers
  (`gate.js`, `import.js`, the execution-context `exec-context-*.js`
  bundles, `conclusion.js`, and `approval-summary.js`), schemars-driven
  type codegen, and the A2 design decision.
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
   environment via AWF — see [`docs/network.md`](docs/network.md). **Scope
   note:** AWF's L7 allowlist wraps *only* the agent's copilot command
   (`awf … --allow-domains … -- '<engine_run>'` in
   `src/compile/agentic_pipeline.rs::run_agent_step`). All other ADO steps —
   binary/bundle downloads, `docker pull`, ACR/NuGet auth (including the
   `supply-chain:` mirror fetches) — run *outside* the sandbox with the build
   agent pool's normal network, so they do **not** need entries in the AWF
   allowlist. Air-gapping the build agent itself from GitHub/GHCR is the agent
   pool's network policy, not AWF.
3. **Tool Allow-listing**: Agents have access to a limited, controlled set of
   tools — see [`docs/tools.md`](docs/tools.md) and
   [`docs/mcp.md`](docs/mcp.md).
4. **Input Sanitization**: Validate and sanitize all inputs before
   transformation — `src/validate.rs` and `src/sanitize.rs`.
5. **Typed path/identifier fields**: When adding a safe-output tool (or any
   code) with a `Params` field that holds a file path, git ref, commit SHA,
   artifact name, or other identifier, type it with a validated newtype from
   `src/secure.rs` (e.g. `RelativeSafePath`, `StrictRelativePath`,
   `GitRefName`, `CommitSha`, `ArtifactName`) instead of a raw `String`. These
   newtypes run the `src/validate.rs` primitives at deserialization time, so
   the security checks cannot be silently forgotten or weakened. Keep
   `validate()` only for cross-field/semantic rules.
6. **Permission Scoping**: Default to minimal permissions, require explicit
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

### Recompile all agentic workflows in the current directory

```bash
# Auto-discovers and recompiles all detected agentic workflows
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
