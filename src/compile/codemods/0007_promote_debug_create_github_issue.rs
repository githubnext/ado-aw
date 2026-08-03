//! `ado-aw-debug.create-issue` -> `safe-outputs.create-github-issue`
//!
//! The GitHub issue tool graduated from the dogfood-only debug surface to a
//! regular safe output. This codemod performs a one-way migration and removes
//! the legacy key entirely; there is no deprecated parser/runtime alias.
//!
//! When the workflow has no explicit SafeOutputs GitHub authentication, the
//! codemod preserves the existing pipeline secret by adding:
//!
//! ```yaml
//! safe-outputs:
//!   github-token: "$(ADO_AW_DEBUG_GITHUB_TOKEN)"
//! ```
//!
//! That value is handled by the regular Stage 3 token path and can be renamed
//! later to the new `ADO_AW_GITHUB_TOKEN` default.

use anyhow::{Result, bail};
use serde_yaml::{Mapping, Value};

use super::{Codemod, CodemodContext};

const INTRODUCED_IN: &str = "0.46.0";

pub static CODEMOD: Codemod = Codemod {
    id: "promote_debug_create_github_issue",
    summary: "ado-aw-debug.create-issue moved to safe-outputs.create-github-issue",
    introduced_in: INTRODUCED_IN,
    apply: apply_codemod,
};

fn key(name: &str) -> Value {
    Value::String(name.to_string())
}

fn apply_codemod(fm: &mut Mapping, _ctx: &CodemodContext) -> Result<bool> {
    let Some(debug) = fm.get(key("ado-aw-debug")) else {
        return Ok(false);
    };
    let Some(debug_map) = debug.as_mapping() else {
        return Ok(false);
    };
    let Some(create_github_issue) = debug_map.get(key("create-issue")).cloned() else {
        return Ok(false);
    };

    let mut migrated_debug = debug_map.clone();
    migrated_debug.remove(key("create-issue"));

    let mut safe_outputs = match fm.get(key("safe-outputs")) {
        Some(value) => value.as_mapping().cloned().ok_or_else(|| {
            anyhow::anyhow!(
                "manual migration required: `safe-outputs` must be a mapping before \
                 moving `ado-aw-debug.create-issue`"
            )
        })?,
        None => Mapping::new(),
    };

    if safe_outputs.contains_key(key("create-github-issue")) {
        bail!(
            "manual migration required: both `ado-aw-debug.create-issue` and \
             `safe-outputs.create-github-issue` are configured"
        );
    }

    if !safe_outputs.contains_key(key("github-token"))
        && !safe_outputs.contains_key(key("github-app"))
    {
        safe_outputs.insert(
            key("github-token"),
            Value::String("$(ADO_AW_DEBUG_GITHUB_TOKEN)".to_string()),
        );
    }
    safe_outputs.insert(key("create-github-issue"), create_github_issue);

    if migrated_debug.is_empty() {
        fm.remove(key("ado-aw-debug"));
    } else {
        fm.insert(key("ado-aw-debug"), Value::Mapping(migrated_debug));
    }
    fm.insert(key("safe-outputs"), Value::Mapping(safe_outputs));
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> CodemodContext {
        CodemodContext {
            compiler_version: INTRODUCED_IN,
            // This codemod migrates a renamed key, not a changed default, so it
            // is unconditional and ignores source provenance entirely.
            source_compiler_version: None,
        }
    }

    fn map(yaml: &str) -> Mapping {
        serde_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn moves_create_github_issue_and_preserves_other_debug_fields() {
        let mut fm = map(
            "ado-aw-debug:\n  skip-integrity: true\n  create-issue:\n    target-repo: octo/repo\n",
        );
        assert!(apply_codemod(&mut fm, &ctx()).unwrap());

        let debug = fm
            .get(key("ado-aw-debug"))
            .and_then(Value::as_mapping)
            .unwrap();
        assert_eq!(debug.get(key("skip-integrity")), Some(&Value::Bool(true)));
        assert!(!debug.contains_key(key("create-issue")));

        let safe_outputs = fm
            .get(key("safe-outputs"))
            .and_then(Value::as_mapping)
            .unwrap();
        assert!(safe_outputs.contains_key(key("create-github-issue")));
        assert_eq!(
            safe_outputs
                .get(key("github-token"))
                .and_then(Value::as_str),
            Some("$(ADO_AW_DEBUG_GITHUB_TOKEN)")
        );
    }

    #[test]
    fn removes_empty_debug_section() {
        let mut fm = map("ado-aw-debug:\n  create-issue:\n    target-repo: octo/repo\n");
        assert!(apply_codemod(&mut fm, &ctx()).unwrap());
        assert!(!fm.contains_key(key("ado-aw-debug")));
    }

    #[test]
    fn preserves_existing_safe_outputs_auth() {
        let mut fm = map(
            "ado-aw-debug:\n  create-issue:\n    target-repo: octo/repo\nsafe-outputs:\n  github-token: $(NEW_TOKEN)\n  noop: {}\n",
        );
        assert!(apply_codemod(&mut fm, &ctx()).unwrap());
        let safe_outputs = fm
            .get(key("safe-outputs"))
            .and_then(Value::as_mapping)
            .unwrap();
        assert_eq!(
            safe_outputs
                .get(key("github-token"))
                .and_then(Value::as_str),
            Some("$(NEW_TOKEN)")
        );
        assert!(safe_outputs.contains_key(key("noop")));
    }

    #[test]
    fn errors_without_mutation_when_new_key_already_exists() {
        let mut fm = map(
            "ado-aw-debug:\n  create-issue:\n    target-repo: old/repo\nsafe-outputs:\n  create-github-issue:\n    target-repo: new/repo\n",
        );
        let snapshot = fm.clone();
        let err = apply_codemod(&mut fm, &ctx()).unwrap_err().to_string();
        assert!(err.contains("both `ado-aw-debug.create-issue`"));
        assert_eq!(fm, snapshot);
    }

    #[test]
    fn is_idempotent() {
        let mut fm = map("ado-aw-debug:\n  create-issue:\n    target-repo: octo/repo\n");
        assert!(apply_codemod(&mut fm, &ctx()).unwrap());
        let snapshot = fm.clone();
        assert!(!apply_codemod(&mut fm, &ctx()).unwrap());
        assert_eq!(fm, snapshot);
    }
}
