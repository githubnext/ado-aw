//! Absent `on:` → explicit `on.push` (all branches)
//!
//! `on:` is the complete declaration of when a pipeline runs, so a source
//! with no `on:` key compiles to a manual / API-queued-only pipeline
//! (`trigger: none` + `pr: none`).
//!
//! Before [`INTRODUCED_IN`] the compiler emitted **no** top-level `trigger:`
//! key in that case, and Azure DevOps reads a missing `trigger:` as *"run CI
//! on every branch"* rather than *"no CI"*. Workflows authored against the
//! old compiler therefore relied on an implicit all-branches push trigger
//! that the new semantics would silently switch off.
//!
//! This codemod pins that legacy behaviour explicitly so existing pipelines
//! keep triggering:
//!
//! ```yaml
//! # (no `on:` key at all)
//! ```
//!
//! becomes
//!
//! ```yaml
//! on:
//!   push:
//!     branches:
//!       include:
//!         - '*'
//! ```
//!
//! # Why this one needs source provenance
//!
//! Unlike a renamed key, the shape being migrated is an *absence*, and that
//! same absence is the valid, intentional spelling of "manual-only" under the
//! new semantics. A permanent codemod would therefore make manual-only
//! pipelines unreachable — every newly authored `on:`-less workflow would have
//! a push trigger injected back into its source on first compile.
//!
//! So it fires only when [`CodemodContext::source_compiler_version`] proves
//! the source predates the change. A source with no committed `.lock.yml`
//! (or one whose header carries no version) is treated as newly authored and
//! left alone. The codemod self-retires once every source has been recompiled.
//!
//! It additionally requires the *running* binary to be at or after
//! [`INTRODUCED_IN`], mirroring `0002_pool_object_form`. That second gate
//! keeps `compile` and `check` consistent: a pre-cutover binary writes a
//! pre-cutover version into the `.lock.yml` it generates, which the
//! source-provenance gate alone would then read back as "old" on the very
//! next `check`, reporting a pending migration for a file that was just
//! compiled. The consequence is that the codemod lies dormant until the
//! cutover release — acceptable because a pre-cutover binary still emits the
//! old trigger shape, so there is nothing to preserve yet.
//!
//! Note this migrates only the `trigger:` half. The old compiler also emitted
//! no `pr:` key, but for Azure Repos — where these pipelines run — the YAML
//! `pr:` block is inert unless a Build Validation branch policy is registered
//! server-side (see `docs/front-matter.md`). Synthesising an `on.pr` here
//! would switch on the whole synthetic-PR machinery, which is a far bigger
//! behaviour change than the one being preserved.

use anyhow::Result;
use serde_yaml::{Mapping, Value};

use super::{Codemod, CodemodContext};

/// Version where `on:` became the complete declaration of when a pipeline
/// runs, and an absent `on:` started compiling to `trigger: none`.
///
/// **Must match the release this ships in.** v0.48.0 shipped the *old*
/// semantics, so this names the next minor — a `feat!` bumps 0.48.0 → 0.49.0.
/// If release-please lands the change under a different version, update this
/// constant, or sources compiled by the last old-semantics release will never
/// be migrated.
pub(crate) const INTRODUCED_IN: &str = "0.49.0";

pub static CODEMOD: Codemod = Codemod {
    id: "explicit_push_trigger",
    summary: "no `on:` no longer means \"CI on every branch\" -> pinned as explicit `on.push`",
    introduced_in: INTRODUCED_IN,
    apply: apply_codemod,
};

/// Trigger keys under `on:` whose presence means the author already made a
/// deliberate choice about when the pipeline runs.
const TRIGGER_KEYS: [&str; 4] = ["push", "pr", "schedule", "pipeline"];

/// Build the `push: { branches: { include: ['*'] } }` value.
fn all_branches_push() -> Value {
    let mut include = Mapping::new();
    include.insert(
        Value::String("include".to_string()),
        Value::Sequence(vec![Value::String("*".to_string())]),
    );
    let mut branches = Mapping::new();
    branches.insert(Value::String("branches".to_string()), Value::Mapping(include));
    Value::Mapping(branches)
}

fn apply_codemod(fm: &mut Mapping, ctx: &CodemodContext) -> Result<bool> {
    // Two independent gates must both hold.
    //
    // 1. The RUNNING binary must actually implement the new semantics.
    //    Before the cutover release the compiler still emits the old
    //    shape, so there is nothing to migrate — and, critically, a
    //    freshly written `.lock.yml` records the running (pre-cutover)
    //    version, which would otherwise make every just-compiled source
    //    look "old" to the very next `ado-aw check`.
    // 2. The SOURCE must predate the cutover. An unparseable or absent
    //    version fails closed (see `crate::version`).
    if !crate::version::is_at_least(ctx.compiler_version, INTRODUCED_IN) {
        return Ok(false);
    }
    let Some(source_version) = ctx.source_compiler_version.as_deref() else {
        return Ok(false);
    };
    if !crate::version::is_older_than(source_version, INTRODUCED_IN) {
        return Ok(false);
    }

    let on_key = Value::String("on".to_string());
    match fm.get_mut(&on_key) {
        // No `on:` at all — the exact shape that used to mean "CI on
        // every branch".
        None => {
            let mut on_map = Mapping::new();
            on_map.insert(Value::String("push".to_string()), all_branches_push());
            fm.insert(on_key, Value::Mapping(on_map));
            Ok(true)
        }
        Some(on_value) => {
            let Some(on_map) = on_value.as_mapping_mut() else {
                // `on:` is present but not a mapping (e.g. `on: null` from a
                // bare `on:` line). Deserialization will reject anything
                // meaningless; do not guess here.
                return Ok(false);
            };
            // Any declared trigger means the author already chose. In
            // particular `on.push` present is what makes this idempotent.
            if TRIGGER_KEYS
                .iter()
                .any(|k| on_map.contains_key(Value::String((*k).to_string())))
            {
                return Ok(false);
            }
            on_map.insert(Value::String("push".to_string()), all_branches_push());
            Ok(true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Context for a source produced by a pre-cutover compiler, running on a
    /// post-cutover binary. Both gates open.
    fn old_ctx() -> CodemodContext {
        post_cutover_ctx(Some("0.48.0"))
    }

    /// Context for a source already recompiled by a post-cutover binary.
    fn new_ctx() -> CodemodContext {
        post_cutover_ctx(Some(INTRODUCED_IN))
    }

    /// Build a context whose *running* binary is at the cutover, so only the
    /// source-provenance gate varies. The crate version is still below
    /// `INTRODUCED_IN` while this ships, so tests cannot rely on
    /// `CodemodContext::for_source`.
    fn post_cutover_ctx(source_version: Option<&str>) -> CodemodContext {
        CodemodContext {
            compiler_version: INTRODUCED_IN,
            source_compiler_version: source_version.map(str::to_string),
        }
    }

    fn map(yaml: &str) -> Mapping {
        serde_yaml::from_str(yaml).unwrap()
    }

    fn push_include(m: &Mapping) -> Vec<String> {
        m.get(Value::String("on".into()))
            .and_then(|o| o.as_mapping())
            .and_then(|o| o.get(Value::String("push".into())))
            .and_then(|p| p.as_mapping())
            .and_then(|p| p.get(Value::String("branches".into())))
            .and_then(|b| b.as_mapping())
            .and_then(|b| b.get(Value::String("include".into())))
            .and_then(|i| i.as_sequence())
            .expect("on.push.branches.include must exist")
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn injects_all_branches_push_when_on_absent_in_old_source() {
        let mut m = map("name: x\ndescription: y\n");
        assert!(apply_codemod(&mut m, &old_ctx()).unwrap());
        assert_eq!(push_include(&m), vec!["*".to_string()]);
    }

    #[test]
    fn injects_push_when_on_present_but_carries_no_trigger() {
        // `on:` with only unrelated/unknown content still relied on the
        // implicit CI default.
        let mut m = map("name: x\non:\n  something-else: true\n");
        assert!(apply_codemod(&mut m, &old_ctx()).unwrap());
        assert_eq!(push_include(&m), vec!["*".to_string()]);
    }

    #[test]
    fn noop_for_new_source_without_on() {
        // The whole point of the gate: under current semantics an absent
        // `on:` is the valid spelling of "manual / API-queued only".
        let mut m = map("name: x\ndescription: y\n");
        assert!(!apply_codemod(&mut m, &new_ctx()).unwrap());
        assert!(!m.contains_key(Value::String("on".into())));
    }

    #[test]
    fn noop_when_source_version_unknown() {
        // No committed lock file -> treat as newly authored.
        let mut m = map("name: x\ndescription: y\n");
        assert!(!apply_codemod(&mut m, &post_cutover_ctx(None)).unwrap());
        assert!(!m.contains_key(Value::String("on".into())));
    }

    #[test]
    fn noop_when_source_version_is_unparseable() {
        let mut m = map("name: x\n");
        assert!(!apply_codemod(&mut m, &post_cutover_ctx(Some("not-a-version"))).unwrap());
    }

    #[test]
    fn migrates_a_prerelease_source_older_than_the_threshold() {
        // Ordering of pre-releases is why this uses `crate::version`
        // rather than a numeric triple.
        let mut m = map("name: x\n");
        assert!(
            apply_codemod(&mut m, &post_cutover_ctx(Some("0.49.0-beta.1"))).unwrap(),
            "0.49.0-beta.1 precedes 0.49.0 and must still be migrated"
        );
    }

    /// The running-binary gate. A pre-cutover binary still emits the old
    /// trigger shape, and — decisively — writes its own pre-cutover version
    /// into every `.lock.yml` it generates. Without this gate that fresh
    /// lock reads back as "old", so `ado-aw check` would report a pending
    /// migration for a source `ado-aw compile` had just written.
    #[test]
    fn dormant_while_the_running_binary_predates_the_cutover() {
        let mut m = map("name: x\ndescription: y\n");
        let ctx = CodemodContext {
            compiler_version: "0.48.0",
            source_compiler_version: Some("0.48.0".to_string()),
        };
        assert!(!apply_codemod(&mut m, &ctx).unwrap());
        assert!(!m.contains_key(Value::String("on".into())));
    }

    /// Guards the release-window transition: once the running binary reaches
    /// the cutover, a source carrying the last old-semantics release's
    /// version must migrate. v0.48.0 shipped the old behaviour, so those
    /// sources are exactly the ones that need pinning.
    #[test]
    fn fires_for_the_release_immediately_before_the_cutover() {
        let mut m = map("name: x\ndescription: y\n");
        let ctx = CodemodContext {
            compiler_version: INTRODUCED_IN,
            source_compiler_version: Some("0.48.0".to_string()),
        };
        assert!(apply_codemod(&mut m, &ctx).unwrap());
        assert_eq!(push_include(&m), vec!["*".to_string()]);
    }

    #[test]
    fn noop_when_schedule_already_declared() {
        // A schedule already compiled to `trigger: none`; nothing changed.
        let mut m = map("name: x\non:\n  schedule: daily around 03:00\n");
        assert!(!apply_codemod(&mut m, &old_ctx()).unwrap());
    }

    #[test]
    fn noop_when_pr_already_declared() {
        let mut m = map("name: x\non:\n  pr:\n    branches:\n      include: [main]\n");
        assert!(!apply_codemod(&mut m, &old_ctx()).unwrap());
    }

    #[test]
    fn noop_when_pipeline_trigger_already_declared() {
        let mut m = map("name: x\non:\n  pipeline:\n    name: Build\n");
        assert!(!apply_codemod(&mut m, &old_ctx()).unwrap());
    }

    #[test]
    fn noop_when_push_already_declared() {
        let mut m = map("name: x\non:\n  push: none\n");
        assert!(!apply_codemod(&mut m, &old_ctx()).unwrap());
        // `push: none` must survive verbatim.
        let on = m
            .get(Value::String("on".into()))
            .unwrap()
            .as_mapping()
            .unwrap();
        assert_eq!(
            on.get(Value::String("push".into())).unwrap().as_str(),
            Some("none")
        );
    }

    #[test]
    fn noop_when_on_is_not_a_mapping() {
        let mut m = map("name: x\non: nonsense\n");
        assert!(!apply_codemod(&mut m, &old_ctx()).unwrap());
    }

    #[test]
    fn idempotent() {
        let mut m = map("name: x\ndescription: y\n");
        assert!(apply_codemod(&mut m, &old_ctx()).unwrap());
        let snapshot = m.clone();
        assert!(
            !apply_codemod(&mut m, &old_ctx()).unwrap(),
            "second run on an already-migrated mapping must be a no-op"
        );
        assert_eq!(m, snapshot, "second run must not mutate the mapping");
    }

    #[test]
    fn migrated_output_deserializes_to_an_all_branches_push_trigger() {
        use crate::compile::types::{FrontMatter, PushTriggerConfig};
        let mut m = map("name: x\ndescription: y\n");
        assert!(apply_codemod(&mut m, &old_ctx()).unwrap());
        let fm: FrontMatter = serde_yaml::from_value(Value::Mapping(m)).expect("must deserialize");
        let push = fm
            .on_config
            .as_ref()
            .and_then(|o| o.push.as_ref())
            .expect("on.push must be set");
        match push {
            PushTriggerConfig::Filtered(f) => {
                let branches = f.branches.as_ref().expect("branches");
                assert_eq!(branches.include, vec!["*".to_string()]);
            }
            PushTriggerConfig::Disabled(_) => panic!("expected the filtered form"),
        }
    }

    #[test]
    fn version_lt_compares_numerically_not_lexically() {
        // Ordering itself is covered by `crate::version`'s own tests; this
        // pins the threshold constant so a future bump is deliberate.
        assert!(crate::version::is_older_than("0.48.0", INTRODUCED_IN));
        assert!(!crate::version::is_older_than(INTRODUCED_IN, INTRODUCED_IN));
    }
}
