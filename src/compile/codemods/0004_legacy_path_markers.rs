//! Legacy directory markers → explicit ADO path expressions.
//!
//! Before the native-IR migration, the compiler folded a fixed
//! replacement list across the **entire** generated YAML — including
//! user-authored `steps:` / `post-steps:` / `setup:` / `teardown:`.
//! That list expanded the directory markers:
//!
//! - `{{ workspace }}` and `{{ working_directory }}` → the resolved
//!   working directory (`$(Build.SourcesDirectory)`,
//!   `$(Build.SourcesDirectory)/$(Build.Repository.Name)`, or
//!   `$(Build.SourcesDirectory)/<alias>`).
//! - `{{ trigger_repo_directory }}` → the trigger ("self") repo dir.
//!
//! After the IR migration these markers flow through verbatim and are
//! no longer substituted. Rather than restore runtime substitution of
//! a fixed-path anchor (an antipattern under multi-checkout, where
//! `$(Build.SourcesDirectory)` is the shared root of every checked-out
//! repo), this codemod migrates existing sources by rewriting each
//! marker to the explicit ADO path expression it resolved to, derived
//! from the source's own `workspace:` / `repos:`.
//!
//! The rewrite is faithful to the legacy whole-YAML fold: it walks
//! **every string scalar** in the front-matter mapping, so markers in
//! any field (not just custom steps) are migrated.
//!
//! Idempotent: resolved expressions contain no `{{ }}`, so a second
//! run finds nothing to rewrite. Detection-based: returns `Ok(false)`
//! when no targeted marker is present, avoiding any `repos:`
//! deserialization on the common path.

use anyhow::Result;
use serde_yaml::{Mapping, Value};

use super::{Codemod, CodemodContext};
use crate::compile::common::{
    contains_template_marker, generate_trigger_repo_directory, lower_repos,
    resolve_working_directory_expr,
};
use crate::compile::types::ReposItem;

/// Marker base names handled by this codemod. `workspace` and
/// `working_directory` resolve to the same working-directory
/// expression; `trigger_repo_directory` resolves to the self-repo dir.
const MARKER_NAMES: &[&str] = &["workspace", "working_directory", "trigger_repo_directory"];

pub static CODEMOD: Codemod = Codemod {
    id: "legacy_path_markers",
    summary: "{{ workspace }}/{{ working_directory }}/{{ trigger_repo_directory }} -> explicit ADO path",
    introduced_in: "0.38.0",
    apply: apply_codemod,
};

fn apply_codemod(fm: &mut Mapping, _ctx: &CodemodContext) -> Result<bool> {
    // Cheap detection first: only deserialize `repos:` and resolve the
    // working directory when at least one targeted marker is present.
    let present = fm.iter().any(|(_k, v)| value_has_any_marker(v, MARKER_NAMES));
    if !present {
        return Ok(false);
    }

    // Derive the checkout-alias list from the (already unified by
    // `m0001_repos_unified`) `repos:` mapping, then resolve the markers
    // exactly as the typed compile path would.
    let checkout = derive_checkout_aliases(fm)?;
    let workspace = fm
        .get(Value::String("workspace".to_string()))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let agent_name = fm
        .get(Value::String("name".to_string()))
        .and_then(|v| v.as_str())
        .unwrap_or("agent");

    let working_directory = resolve_working_directory_expr(&workspace, &checkout, agent_name)?;
    let trigger_repo_directory = generate_trigger_repo_directory(&checkout);

    let replacements: Vec<(&str, String)> = vec![
        ("workspace", working_directory.clone()),
        ("working_directory", working_directory),
        ("trigger_repo_directory", trigger_repo_directory),
    ];

    let mut changed = false;
    for (_k, v) in fm.iter_mut() {
        changed |= replace_in_value(v, &replacements);
    }
    Ok(changed)
}

/// Derive the checked-out repository aliases from the untyped `repos:`
/// mapping, reusing the same lowering the typed path uses so the two
/// cannot drift. Returns an empty list when `repos:` is absent (the
/// single-checkout case, where `$(Build.SourcesDirectory)` is the repo
/// root).
fn derive_checkout_aliases(fm: &Mapping) -> Result<Vec<String>> {
    let Some(repos_val) = fm.get(Value::String("repos".to_string())) else {
        return Ok(Vec::new());
    };
    let items: Vec<ReposItem> = serde_yaml::from_value(repos_val.clone()).map_err(|e| {
        anyhow::anyhow!("failed to read `repos:` while migrating legacy path markers: {e}")
    })?;
    let (_repositories, checkout, _fetch) = lower_repos(&items)?;
    Ok(checkout)
}

/// Recursively replace every targeted marker in the string scalars of
/// `v`. Keys are left untouched. Returns whether any scalar changed.
fn replace_in_value(v: &mut Value, replacements: &[(&str, String)]) -> bool {
    match v {
        Value::String(s) => {
            let mut updated = s.clone();
            for (name, repl) in replacements {
                updated = replace_marker(&updated, name, repl);
            }
            if &updated != s {
                *s = updated;
                true
            } else {
                false
            }
        }
        Value::Sequence(seq) => {
            let mut changed = false;
            for item in seq.iter_mut() {
                changed |= replace_in_value(item, replacements);
            }
            changed
        }
        Value::Mapping(m) => {
            let mut changed = false;
            for (_k, val) in m.iter_mut() {
                changed |= replace_in_value(val, replacements);
            }
            changed
        }
        _ => false,
    }
}

/// Whether any of `names` appears as a `{{ name }}` marker in `v`'s
/// string scalars (recursive). Whitespace inside the braces is ignored
/// so both `{{ workspace }}` and `{{workspace}}` are detected.
fn value_has_any_marker(v: &Value, names: &[&str]) -> bool {
    match v {
        Value::String(s) => names.iter().any(|n| contains_template_marker(s, n)),
        Value::Sequence(seq) => seq.iter().any(|x| value_has_any_marker(x, names)),
        Value::Mapping(m) => m.iter().any(|(_k, val)| value_has_any_marker(val, names)),
        _ => false,
    }
}

/// Replace every `{{ name }}` marker (interior whitespace ignored) in
/// `input` with `repl`. Non-matching `{{ ... }}` spans (e.g.
/// `{{#runtime-import ...}}` or `${{ parameters.x }}`) are preserved.
fn replace_marker(input: &str, name: &str, repl: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i..].starts_with("{{") {
            // Skip `${{ ... }}` (ADO template expression): it is not a legacy
            // marker, and substituting it would leave a stray `$` before the
            // replacement (e.g. `$$(Build.SourcesDirectory)`). Mirrors the
            // guard in `contains_template_marker`.
            let preceded_by_dollar = i > 0 && input.as_bytes()[i - 1] == b'$';
            let start = i + 2;
            if !preceded_by_dollar
                && let Some(close) = input[start..].find("}}")
                && input[start..start + close].trim() == name
            {
                out.push_str(repl);
                i = start + close + 2;
                continue;
            }
        }
        let Some(ch) = input[i..].chars().next() else {
            break;
        };
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> CodemodContext {
        CodemodContext {
            compiler_version: "0.38.0",
        }
    }

    fn fm_from(yaml: &str) -> Mapping {
        serde_yaml::from_str(yaml).expect("parse front matter")
    }

    fn step_script(fm: &Mapping) -> String {
        let steps = fm.get(Value::String("steps".to_string())).expect("steps");
        let first = steps.as_sequence().expect("seq")[0]
            .as_mapping()
            .expect("map");
        first
            .get(Value::String("script".to_string()))
            .and_then(|v| v.as_str())
            .expect("script")
            .to_string()
    }

    #[test]
    fn noop_when_no_markers() {
        let mut fm = fm_from("name: a\ndescription: d\nsteps:\n  - script: echo hi\n");
        let snapshot = fm.clone();
        let changed = apply_codemod(&mut fm, &ctx()).expect("apply");
        assert!(!changed);
        assert_eq!(fm, snapshot);
    }

    #[test]
    fn workspace_marker_single_checkout_resolves_to_root() {
        // No additional repos → $(Build.SourcesDirectory) is the repo root.
        let mut fm = fm_from(
            "name: a\ndescription: d\nsteps:\n  - script: cd {{ workspace }} && ls\n",
        );
        let changed = apply_codemod(&mut fm, &ctx()).expect("apply");
        assert!(changed);
        assert_eq!(step_script(&fm), "cd $(Build.SourcesDirectory) && ls");
    }

    #[test]
    fn no_space_variant_is_migrated() {
        let mut fm =
            fm_from("name: a\ndescription: d\nsteps:\n  - script: cd {{workspace}}\n");
        let changed = apply_codemod(&mut fm, &ctx()).expect("apply");
        assert!(changed);
        assert_eq!(step_script(&fm), "cd $(Build.SourcesDirectory)");
    }

    #[test]
    fn working_directory_alias_resolves_same_as_workspace() {
        let mut fm = fm_from(
            "name: a\ndescription: d\nsteps:\n  - script: echo {{ working_directory }}\n",
        );
        apply_codemod(&mut fm, &ctx()).expect("apply");
        assert_eq!(step_script(&fm), "echo $(Build.SourcesDirectory)");
    }

    #[test]
    fn multi_checkout_workspace_repo_resolves_to_self_subfolder() {
        let mut fm = fm_from(
            "name: a\ndescription: d\nworkspace: repo\nrepos:\n  - org/other\nsteps:\n  - script: cd {{ workspace }}\n",
        );
        let changed = apply_codemod(&mut fm, &ctx()).expect("apply");
        assert!(changed);
        assert_eq!(
            step_script(&fm),
            "cd $(Build.SourcesDirectory)/$(Build.Repository.Name)"
        );
    }

    #[test]
    fn workspace_alias_resolves_to_alias_subfolder() {
        let mut fm = fm_from(
            "name: a\ndescription: d\nworkspace: other\nrepos:\n  - org/other\nsteps:\n  - script: cd {{ workspace }}\n",
        );
        apply_codemod(&mut fm, &ctx()).expect("apply");
        assert_eq!(step_script(&fm), "cd $(Build.SourcesDirectory)/other");
    }

    #[test]
    fn trigger_repo_directory_marker_multi_checkout() {
        let mut fm = fm_from(
            "name: a\ndescription: d\nrepos:\n  - org/other\nsteps:\n  - script: cat {{ trigger_repo_directory }}/file\n",
        );
        apply_codemod(&mut fm, &ctx()).expect("apply");
        assert_eq!(
            step_script(&fm),
            "cat $(Build.SourcesDirectory)/$(Build.Repository.Name)/file"
        );
    }

    #[test]
    fn preserves_runtime_import_and_parameter_spans() {
        let mut fm = fm_from(
            "name: a\ndescription: d\nsteps:\n  - script: \"${{ parameters.x }} {{#runtime-import foo.md}} {{ workspace }}\"\n",
        );
        apply_codemod(&mut fm, &ctx()).expect("apply");
        assert_eq!(
            step_script(&fm),
            "${{ parameters.x }} {{#runtime-import foo.md}} $(Build.SourcesDirectory)"
        );
    }

    #[test]
    fn migrates_post_steps_and_setup_and_teardown() {
        let mut fm = fm_from(
            "name: a\ndescription: d\n\
             setup:\n  - script: a {{ workspace }}\n\
             steps:\n  - script: b {{ workspace }}\n\
             post-steps:\n  - script: c {{ workspace }}\n\
             teardown:\n  - script: e {{ workspace }}\n",
        );
        let changed = apply_codemod(&mut fm, &ctx()).expect("apply");
        assert!(changed);
        for (key, prefix) in [
            ("setup", "a"),
            ("steps", "b"),
            ("post-steps", "c"),
            ("teardown", "e"),
        ] {
            let seq = fm
                .get(Value::String(key.to_string()))
                .and_then(|v| v.as_sequence())
                .expect("seq");
            let script = seq[0]
                .as_mapping()
                .and_then(|m| m.get(Value::String("script".to_string())))
                .and_then(|v| v.as_str())
                .expect("script");
            assert_eq!(script, format!("{prefix} $(Build.SourcesDirectory)"));
        }
    }

    #[test]
    fn idempotent_second_run_is_noop() {
        let mut fm = fm_from(
            "name: a\ndescription: d\nsteps:\n  - script: cd {{ workspace }}\n",
        );
        let changed1 = apply_codemod(&mut fm, &ctx()).expect("first");
        assert!(changed1);
        let snapshot = fm.clone();
        let changed2 = apply_codemod(&mut fm, &ctx()).expect("second");
        assert!(!changed2, "second run must be a no-op");
        assert_eq!(fm, snapshot);
    }

    #[test]
    fn marker_present_ignores_other_spans() {
        assert!(contains_template_marker("a {{ workspace }} b", "workspace"));
        assert!(contains_template_marker("{{workspace}}", "workspace"));
        assert!(!contains_template_marker("{{#runtime-import x}}", "workspace"));
        assert!(!contains_template_marker("${{ parameters.x }}", "workspace"));
        assert!(!contains_template_marker("no markers here", "workspace"));
    }

    #[test]
    fn dollar_template_expression_is_not_a_marker() {
        // `${{ workspace }}` is an ADO template expression, not a legacy
        // marker: it must not be detected or substituted (substituting would
        // leave a stray `$` before the replacement).
        assert!(!contains_template_marker("${{ workspace }}", "workspace"));
        assert!(!contains_template_marker("a ${{ workspace }} b", "workspace"));
        assert_eq!(
            replace_marker("${{ workspace }}", "workspace", "REPL"),
            "${{ workspace }}"
        );
        // A bare (non-dollar) marker sitting alongside a `${{ }}` expression is
        // still migrated.
        assert!(contains_template_marker("${{ p }} {{ workspace }}", "workspace"));
        assert_eq!(
            replace_marker("${{ p }} {{ workspace }}", "workspace", "REPL"),
            "${{ p }} REPL"
        );
    }

    #[test]
    fn nested_marker_detection_matches_replacement() {
        // Regression: `contains_template_marker` (the early-exit gate) and
        // `replace_marker` must agree on doubly-nested input. Previously the
        // gate jumped past the outer `}}` and missed the inner marker while
        // the replacer found and substituted it — leaving the gate returning
        // `false` for a value that would actually be rewritten.
        let input = "{{ bad {{ workspace }} }}";
        assert!(contains_template_marker(input, "workspace"));
        // The outer `{{ bad ... }}` span does not match because its interior
        // (`"bad {{ workspace"`) is not the marker name. Only the inner
        // `{{ workspace }}` is replaced, leaving the surrounding braces intact.
        assert_eq!(
            replace_marker(input, "workspace", "REPL"),
            "{{ bad REPL }}",
            "replace_marker should substitute only the inner {{ workspace }} marker"
        );
    }
}
