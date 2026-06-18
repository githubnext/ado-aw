//! Shared helpers for the typed task builders in this module.
//!
//! Every task in `tasks/` is a **builder struct**: `new(<required>)` plus one
//! typed chained setter per optional input plus `into_step(self) -> TaskStep`.
//! Only fields that were explicitly set emit an ADO `inputs:` entry, so the
//! generated YAML stays minimal and matches the task's own defaults.
//!
//! Constrained ADO input values (e.g. `archiveType`, `releaseType`) are modeled
//! as enums colocated with the task that uses them; each enum exposes
//! `as_ado_str()` returning the exact token ADO expects. Bool-string inputs are
//! `Option<bool>` and lowered through [`bool_input`].

use crate::compile::ir::step::TaskStep;

/// Lower a Rust `bool` to the `"true"` / `"false"` string ADO task inputs use.
pub(crate) fn bool_input(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

/// Insert an optional string input only when present. Used by command-dispatch
/// tasks whose `into_step` lowers many per-variant optionals.
pub(crate) fn push_opt(t: &mut TaskStep, key: &str, value: Option<String>) {
    if let Some(v) = value {
        t.inputs.insert(key.to_string(), v);
    }
}

/// Insert an optional bool-string input only when present.
pub(crate) fn push_bool(t: &mut TaskStep, key: &str, value: Option<bool>) {
    if let Some(v) = value {
        t.inputs.insert(key.to_string(), bool_input(v).to_string());
    }
}
