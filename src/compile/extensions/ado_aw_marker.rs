//! Always-on ado-aw marker extension.
//!
//! Injects a single informational step into the Setup job of every
//! compiled pipeline. The step's bash body carries a machine-readable
//! JSON metadata blob keyed by a `# ado-aw-metadata:` prefix, plus a
//! runtime `echo` for build-log visibility.
//!
//! Why a step (and not a top-of-file comment): ADO's Pipeline Preview
//! API strips top-of-document leading comments during YAML expansion
//! (verified empirically against live def 2434 in `msazuresphere/4x4`).
//! Comments embedded inside step bodies are preserved verbatim. The
//! marker has to live inside a step body to survive Preview-driven
//! discovery.
//!
//! Why JSON inside the marker: forward-compatible schema. New fields
//! (e.g., compiler-derived secrets list) can be added without breaking
//! older parsers, mirroring gh-aw's `# gh-aw-metadata: {...}` shape.

use super::{CompileContext, CompilerExtension, ExtensionPhase};
use anyhow::Result;

// ─── ado-aw marker (always-on, internal) ─────────────────────────────

/// Always-on internal extension that embeds machine-readable
/// `# ado-aw-metadata: {…}` JSON inside an injected Setup-job step.
///
/// The metadata is the canonical surface consumed by Preview-driven
/// project-scope discovery in [`crate::ado`]. Discovery enumerates ADO
/// definitions, expands each via the Pipeline Preview API, and greps
/// the result for this marker.
pub struct AdoAwMarkerExtension;

impl CompilerExtension for AdoAwMarkerExtension {
    fn name(&self) -> &str {
        "ado-aw-marker"
    }

    fn phase(&self) -> ExtensionPhase {
        // Tool phase keeps the marker step grouped with the other
        // always-on internal extensions (GitHub, SafeOutputs). The
        // marker has no execution dependency on anything else; the
        // phase choice is purely about emit order.
        ExtensionPhase::Tool
    }

    fn setup_steps(&self, ctx: &CompileContext) -> Result<Vec<String>> {
        // In unit-test contexts that build a CompileContext without an
        // input_path (e.g. CompileContext::for_test), skip the marker.
        // Production paths always populate input_path via
        // CompileContext::new.
        let Some(input_path) = ctx.input_path else {
            return Ok(vec![]);
        };

        let source = super::super::common::normalize_source_path(input_path);
        let version = env!("CARGO_PKG_VERSION");
        let target = ctx.front_matter.target.as_str();

        let metadata_json = serde_json::json!({
            "schema": 1,
            "source": source,
            "version": version,
            "target": target,
        })
        .to_string();

        // The `# ado-aw-metadata:` line is the parse target for
        // discovery. The `echo` makes the same information visible in
        // the build log at runtime, which is a free human-discoverability
        // bonus and costs nothing because the step runs in milliseconds.
        //
        // Single quotes around the echo argument keep the JSON literal
        // intact; the metadata contains no single quotes (JSON strings
        // escape them via \u0027 if ever needed).
        let step = format!(
            "- bash: |\n    \
                # ado-aw-metadata: {metadata}\n    \
                echo 'ado-aw metadata: source={source} version={version} target={target}'\n  \
            displayName: \"ado-aw\"\n",
            metadata = metadata_json,
            source = source,
            version = version,
            target = target,
        );

        Ok(vec![step])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::extensions::CompileContext;
    use crate::compile::types::FrontMatter;
    use std::path::Path;

    fn parse_fm(yaml: &str) -> FrontMatter {
        serde_yaml::from_str(yaml).expect("front matter parses")
    }

    #[test]
    fn returns_no_step_when_input_path_absent() {
        let fm = parse_fm("name: t\ndescription: x\n");
        let ctx = CompileContext::for_test(&fm);
        let steps = AdoAwMarkerExtension.setup_steps(&ctx).unwrap();
        assert!(steps.is_empty(), "expected no marker when input_path is None");
    }

    #[test]
    fn emits_single_step_with_canonical_displayname() {
        // Production path: CompileContext::new populates input_path.
        // Simulate by hand for this unit test.
        let fm = parse_fm("name: t\ndescription: x\n");
        let input_path = Path::new("agents/foo.md");
        let ctx = CompileContext {
            agent_name: &fm.name,
            front_matter: &fm,
            ado_context: None,
            engine: crate::engine::Engine::Copilot,
            compile_dir: None,
            input_path: Some(input_path),
        };
        let steps = AdoAwMarkerExtension.setup_steps(&ctx).unwrap();
        assert_eq!(steps.len(), 1);
        let step = &steps[0];
        assert!(step.contains("displayName: \"ado-aw\""), "step missing displayName:\n{step}");
        assert!(step.contains("# ado-aw-metadata:"), "step missing JSON marker line:\n{step}");
        assert!(step.contains("\"source\":\"agents/foo.md\""), "step missing source field:\n{step}");
        assert!(step.contains("\"target\":\"standalone\""), "step missing target field:\n{step}");
        assert!(step.contains("\"schema\":1"), "step missing schema field:\n{step}");
    }

    #[test]
    fn target_field_reflects_front_matter() {
        for (raw_target, expected) in [
            ("standalone", "standalone"),
            ("1es", "1es"),
            ("job", "job"),
            ("stage", "stage"),
        ] {
            let yaml = format!("name: t\ndescription: x\ntarget: {raw_target}\n");
            let fm = parse_fm(&yaml);
            let input_path = Path::new("agents/foo.md");
            let ctx = CompileContext {
                agent_name: &fm.name,
                front_matter: &fm,
                ado_context: None,
                engine: crate::engine::Engine::Copilot,
                compile_dir: None,
                input_path: Some(input_path),
            };
            let steps = AdoAwMarkerExtension.setup_steps(&ctx).unwrap();
            assert_eq!(steps.len(), 1, "target={raw_target}");
            assert!(
                steps[0].contains(&format!("\"target\":\"{expected}\"")),
                "expected target={expected} in step (raw input {raw_target}):\n{}",
                steps[0]
            );
        }
    }
}
