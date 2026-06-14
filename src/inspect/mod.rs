//! Inspection commands: typed-IR queries over agent source files.
//!
//! This module is the home for every read-only command that loads an
//! agent's `.md`, builds the typed [`crate::compile::ir::Pipeline`]
//! IR, and answers a question about it without producing any YAML on
//! disk.
//!
//! Layout follows `src/audit/`:
//!
//! - `cli.rs` — dispatchers for the public CLI subcommands.
//! - `graph_query.rs` — the `ado-aw graph` family (text/json/dot,
//!   `graph deps`, `graph outputs`).
//!
//! Future siblings (called out in the implementation plan, not yet
//! landed):
//!
//! - `trace.rs` — `ado-aw trace`: joins build telemetry from
//!   [`crate::audit`] with the IR graph.
//! - `whatif.rs` — `ado-aw whatif`: static reachability ("which jobs
//!   skip if X fails?") from the typed `Condition` + `depends_on`.
//! - `lint.rs` — `ado-aw lint`: structural checks layered on top of
//!   the compile-stage validators.
//! - `catalog.rs` — `ado-aw catalog`: programmatic listing of
//!   in-tree registries (safe-outputs, runtimes, tools, engines,
//!   models).

pub mod catalog;
pub mod cli;
pub mod graph_deps;
pub mod graph_outputs;
pub mod graph_query;
pub mod lint;
pub mod trace;
pub mod whatif;

pub use cli::{
    CatalogOptions, GraphDepsOptions, GraphFormat, GraphOptions, GraphOutputsOptions,
    InspectOptions, LintOptions, TraceOptions, WhatIfOptions, build_catalog, build_graph_deps,
    build_graph_dump, build_graph_outputs, build_graph_summary, build_inspect, build_lint,
    build_trace, build_whatif, dispatch_catalog, dispatch_graph, dispatch_graph_deps,
    dispatch_graph_outputs, dispatch_inspect, dispatch_lint, dispatch_trace, dispatch_whatif,
};
pub use graph_deps::GraphDepsDirection;
