//! Checkout-aware path-layout validation (warning-only).
//!
//! Azure DevOps lays out checked-out repositories in two distinct
//! shapes depending on how many repos are checked out:
//!
//! - **Single checkout** (only the trigger repo): `$(Build.SourcesDirectory)`
//!   *is* the trigger repo root.
//! - **Multi-checkout** (≥1 additional repo): every repo, including the
//!   trigger repo, lives in a subfolder of `$(Build.SourcesDirectory)`
//!   named after its alias — the trigger repo under
//!   `$(Build.Repository.Name)`, each additional repo under its `repos:`
//!   alias.
//!
//! When authors hand-write paths anchored at `$(Build.SourcesDirectory)`
//! (or reference repos via `{{#runtime-import …}}`), it is easy to point
//! at a path that will not exist under the resolved layout. This pass
//! surfaces those mistakes as **warnings** — it never fails the compile,
//! because the compiler cannot always resolve a path (e.g. the trigger
//! repo's literal name behind `$(Build.Repository.Name)`), and false
//! positives must not block builds.
//!
//! It also warns when deprecated directory markers
//! (`{{ workspace }}` / `{{ working_directory }}` /
//! `{{ trigger_repo_directory }}`) survive in the **agent body**: the
//! `legacy_path_markers` codemod only migrates front matter, and these
//! markers are no longer substituted at runtime.

use serde_yaml::Value;

use crate::compile::common::contains_template_marker;
use crate::compile::types::FrontMatter;

/// Literal prefix that anchors a path at the checkout root, including the
/// trailing separator (so a bare `$(Build.SourcesDirectory)` root
/// reference is not treated as having a sub-segment).
const SOURCES_DIR_PREFIX: &str = "$(Build.SourcesDirectory)/";

/// The runtime macro under which the trigger ("self") repo is checked
/// out in multi-checkout mode.
const SELF_REPO_SEGMENT: &str = "$(Build.Repository.Name)";

/// Deprecated directory markers that the `legacy_path_markers` codemod
/// migrates in front matter but cannot touch in the agent body.
const DEPRECATED_MARKERS: &[&str] = &["workspace", "working_directory", "trigger_repo_directory"];

/// Collect checkout-aware path-layout warnings for a compiled workflow.
///
/// Warning-only: the returned strings are advisory. Returns an empty
/// vector when nothing looks wrong. Messages are de-duplicated.
pub(crate) fn collect_path_layout_warnings(front_matter: &FrontMatter, markdown_body: &str) -> Vec<String> {
    let mut warnings: Vec<String> = Vec::new();

    let checked_out: Vec<&str> = front_matter.checkout.iter().map(String::as_str).collect();
    let multi = !checked_out.is_empty();
    // Repos declared in `repos:` but not checked out (`checkout: false`).
    let declared_not_checked_out: Vec<&str> = front_matter
        .repositories
        .iter()
        .map(|r| r.repository.as_str())
        .filter(|alias| !checked_out.iter().any(|c| c == alias))
        .collect();

    // 1. `$(Build.SourcesDirectory)/<seg>` references in custom steps.
    //
    // Deliberately narrowed to the executable step blocks
    // (`setup/steps/post_steps/teardown`). The codemod migrates every
    // front-matter string scalar, but only step scalars are interpreted as
    // filesystem paths at runtime, so a stray `$(Build.SourcesDirectory)/…`
    // in a non-step field (e.g. `description:`) is harmless and intentionally
    // not flagged here.
    let mut step_scalars: Vec<&str> = Vec::new();
    for block in [
        &front_matter.setup,
        &front_matter.steps,
        &front_matter.post_steps,
        &front_matter.teardown,
    ] {
        for value in block {
            collect_string_scalars(value, &mut step_scalars);
        }
    }
    for scalar in &step_scalars {
        for seg in sources_dir_segments(scalar) {
            if declared_not_checked_out.contains(&seg.as_str()) {
                warnings.push(format!(
                    "step references `$(Build.SourcesDirectory)/{seg}`, but repository `{seg}` is \
                     declared in `repos:` with `checkout: false`; its sources will not be present \
                     at that path. Set `checkout: true` or remove the reference."
                ));
            } else if !multi && seg == SELF_REPO_SEGMENT {
                warnings.push(
                    "step references `$(Build.SourcesDirectory)/$(Build.Repository.Name)`, but with \
                     only the trigger repository checked out `$(Build.SourcesDirectory)` already IS \
                     the repository root; that subfolder will not exist. Use \
                     `$(Build.SourcesDirectory)` directly."
                        .to_string(),
                );
            }
        }
    }

    // 2. Runtime-import paths whose first segment is a declared-but-not-checked-out repo.
    for seg in runtime_import_first_segments(markdown_body) {
        if declared_not_checked_out.contains(&seg.as_str()) {
            warnings.push(format!(
                "`{{{{#runtime-import {seg}/…}}}}` targets repository `{seg}`, which is declared in \
                 `repos:` with `checkout: false`; it will not be present in the workspace at \
                 runtime. Set `checkout: true` for that repository."
            ));
        }
    }

    // 3. Deprecated directory markers surviving in the agent body.
    for marker in DEPRECATED_MARKERS {
        if contains_template_marker(markdown_body, marker) {
            warnings.push(format!(
                "deprecated directory marker `{{{{ {marker} }}}}` found in the agent body; it is no \
                 longer substituted (the migration codemod only rewrites front matter). Replace it \
                 with an explicit `$(Build.SourcesDirectory)…` path."
            ));
        }
    }

    warnings.sort();
    warnings.dedup();
    warnings
}

/// Recursively collect borrowed string scalars from a YAML value
/// (mapping values and sequence items; keys are ignored).
fn collect_string_scalars<'a>(value: &'a Value, out: &mut Vec<&'a str>) {
    match value {
        Value::String(s) => out.push(s.as_str()),
        Value::Sequence(seq) => {
            for item in seq {
                collect_string_scalars(item, out);
            }
        }
        Value::Mapping(m) => {
            for (_k, v) in m {
                collect_string_scalars(v, out);
            }
        }
        _ => {}
    }
}

/// Extract the first path segment after each `$(Build.SourcesDirectory)/`
/// occurrence in `s`. The segment runs up to the next path separator,
/// whitespace, or quote.
fn sources_dir_segments(s: &str) -> Vec<String> {
    let mut segs = Vec::new();
    let mut i = 0;
    while let Some(pos) = s[i..].find(SOURCES_DIR_PREFIX) {
        let start = i + pos + SOURCES_DIR_PREFIX.len();
        let seg: String = s[start..]
            .chars()
            .take_while(|&c| c != '/' && !c.is_whitespace() && c != '"' && c != '\'' && c != '\\')
            .collect();
        // Advance past the extracted segment. `i = start` (immediately after
        // the prefix) would re-scan the extracted span on the next iteration
        // for a back-to-back double prefix
        // (`$(Build.SourcesDirectory)/$(Build.SourcesDirectory)/foo`), pushing
        // the same span twice; `start + seg.len()` is the intended
        // advancement and reports each segment once. When `seg` is empty (the
        // char right after the prefix is a stopper like a space or quote) this
        // is `start + 0 = start`, which still makes progress: the following
        // `find` starts past the just-matched prefix and cannot re-match it,
        // so there is no infinite-loop risk.
        i = start + seg.len();
        if !seg.is_empty() {
            segs.push(seg);
        }
    }
    segs
}

/// Extract the first path segment of every `{{#runtime-import path}}` /
/// `{{#runtime-import? path}}` marker in `body`.
fn runtime_import_first_segments(body: &str) -> Vec<String> {
    const KEY: &str = "{{#runtime-import";
    let mut segs = Vec::new();
    let mut i = 0;
    while let Some(pos) = body[i..].find(KEY) {
        let after = i + pos + KEY.len();
        match body[after..].find("}}") {
            Some(close) => {
                let inner = body[after..after + close].trim();
                // Optional imports are written `{{#runtime-import? path}}`.
                let inner = inner.strip_prefix('?').unwrap_or(inner).trim();
                if let Some(path) = inner.split_whitespace().next()
                    && let Some(first) = path.split('/').next()
                    && !first.is_empty()
                {
                    segs.push(first.to_string());
                }
                i = after + close + 2;
            }
            None => break,
        }
    }
    segs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fm(yaml: &str) -> FrontMatter {
        let mut fm: FrontMatter = serde_yaml::from_str(yaml).expect("parse front matter");
        // Lower repos the way the compile pipeline does.
        if !fm.repos.is_empty() {
            let (repositories, checkout, checkout_fetch) =
                crate::compile::common::lower_repos(&fm.repos).expect("lower repos");
            fm.repositories = repositories;
            fm.checkout = checkout;
            fm.checkout_fetch = checkout_fetch;
        }
        fm
    }

    #[test]
    fn no_warnings_for_clean_single_checkout() {
        let fm = fm("name: a\ndescription: d\nsteps:\n  - script: cd $(Build.SourcesDirectory)\n");
        assert!(collect_path_layout_warnings(&fm, "body").is_empty());
    }

    #[test]
    fn warns_on_self_subfolder_in_single_checkout() {
        let fm = fm(
            "name: a\ndescription: d\nsteps:\n  - script: cd $(Build.SourcesDirectory)/$(Build.Repository.Name)\n",
        );
        let w = collect_path_layout_warnings(&fm, "body");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("already IS the repository root"), "{w:?}");
    }

    #[test]
    fn no_self_subfolder_warning_in_multi_checkout() {
        let fm = fm(
            "name: a\ndescription: d\nrepos:\n  - org/other\nsteps:\n  - script: cd $(Build.SourcesDirectory)/$(Build.Repository.Name)\n",
        );
        assert!(collect_path_layout_warnings(&fm, "body").is_empty());
    }

    #[test]
    fn warns_on_reference_to_not_checked_out_repo() {
        let fm = fm(
            "name: a\ndescription: d\nrepos:\n  - name: org/other\n    checkout: false\nsteps:\n  - script: cat $(Build.SourcesDirectory)/other/file\n",
        );
        let w = collect_path_layout_warnings(&fm, "body");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("`checkout: false`"), "{w:?}");
        assert!(w[0].contains("other"), "{w:?}");
    }

    #[test]
    fn no_warning_for_checked_out_alias_reference() {
        let fm = fm(
            "name: a\ndescription: d\nrepos:\n  - org/other\nsteps:\n  - script: cat $(Build.SourcesDirectory)/other/file\n",
        );
        assert!(collect_path_layout_warnings(&fm, "body").is_empty());
    }

    #[test]
    fn warns_on_runtime_import_to_not_checked_out_repo() {
        let fm = fm(
            "name: a\ndescription: d\nrepos:\n  - name: org/other\n    checkout: false\n",
        );
        let body = "Read {{#runtime-import other/docs/policy.md}} please";
        let w = collect_path_layout_warnings(&fm, body);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("runtime-import"), "{w:?}");
    }

    #[test]
    fn warns_on_deprecated_marker_in_body() {
        let fm = fm("name: a\ndescription: d\n");
        let body = "Run inside {{ workspace }} now.";
        let w = collect_path_layout_warnings(&fm, body);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("agent body"), "{w:?}");
    }

    #[test]
    fn deduplicates_repeated_warnings() {
        let fm = fm(
            "name: a\ndescription: d\nsteps:\n  - script: cd $(Build.SourcesDirectory)/$(Build.Repository.Name)\n  - script: ls $(Build.SourcesDirectory)/$(Build.Repository.Name)\n",
        );
        let w = collect_path_layout_warnings(&fm, "body");
        assert_eq!(w.len(), 1, "{w:?}");
    }

    #[test]
    fn sources_dir_segments_extraction() {
        assert_eq!(
            sources_dir_segments("cd $(Build.SourcesDirectory)/foo/bar && ls"),
            vec!["foo".to_string()]
        );
        assert!(sources_dir_segments("cd $(Build.SourcesDirectory)").is_empty());
        assert_eq!(
            sources_dir_segments("$(Build.SourcesDirectory)/$(Build.Repository.Name)/x"),
            vec![SELF_REPO_SEGMENT.to_string()]
        );
        // Back-to-back double prefix: the inner `$(Build.SourcesDirectory)`
        // is extracted as the segment and the loop advances past it (rather
        // than re-scanning from just after the outer prefix), so the segment
        // is reported exactly once with no duplicate/re-extraction.
        assert_eq!(
            sources_dir_segments("$(Build.SourcesDirectory)/$(Build.SourcesDirectory)/foo"),
            vec!["$(Build.SourcesDirectory)".to_string()]
        );
    }

    #[test]
    fn runtime_import_segments_extraction() {
        assert_eq!(
            runtime_import_first_segments("{{#runtime-import other/x.md}}"),
            vec!["other".to_string()]
        );
        assert_eq!(
            runtime_import_first_segments("{{#runtime-import? other/x.md}}"),
            vec!["other".to_string()]
        );
        assert!(runtime_import_first_segments("no imports").is_empty());
    }
}
