/// Shared audit data types for `ado-aw audit`.
///
/// This module defines the public report model that analyzers populate and renderers
/// consume for single-build Azure DevOps audit output.
pub mod analyzers;
pub mod cache;
pub mod cli;
pub mod findings;
pub mod model;
pub mod render;
pub mod url;

pub use cli::{AuditOptions, dispatch};
#[allow(unused_imports)]
pub use model::*;
