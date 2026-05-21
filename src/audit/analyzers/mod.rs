//! Analyzers that consume artifact files produced by ado-aw pipelines
//! and populate sections of [`crate::audit::model::AuditData`].
//!
//! Each submodule owns one signal: firewall, mcp, otel, safe-outputs,
//! detection, missing-tools/data/noops, build timeline.

pub mod detection;
pub mod firewall;
pub mod jobs;
pub mod mcp;
pub mod missing;
pub mod otel;
pub mod policy;
pub mod safe_outputs;
