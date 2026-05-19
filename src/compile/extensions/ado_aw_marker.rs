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

    fn prepare_steps(&self, ctx: &CompileContext) -> Vec<String> {
        // Inject the marker step into the Agent job's prepare phase
        // (NOT a separate Setup job). Setup-job injection would force
        // every compiled pipeline to spin up an extra agent pool job
        // just to emit a metadata comment — wasteful for pipelines
        // that have no other reason to need a Setup job. prepare_steps
        // lands inside the always-present Agent job's
        // `{{ prepare_steps }}` block, so it costs zero extra
        // jobs/agents/pool time.
        //
        // In unit-test contexts that build a CompileContext without an
        // input_path (e.g. CompileContext::for_test), skip the marker.
        // Production paths always populate input_path via
        // CompileContext::new.
        let Some(input_path) = ctx.input_path else {
            return vec![];
        };

        let source = super::super::common::normalize_source_path(input_path);
        let version = env!("CARGO_PKG_VERSION");
        let target = ctx.front_matter.target.as_str();

        // ADO origin of the source markdown — disambiguates the
        // `source` field when two repos in the same project happen to
        // have files of the same name (e.g. both define `agents/foo.md`).
        // Lower-cased so case-insensitive ADO identifiers compare cleanly.
        // Empty strings when no ADO context could be inferred — production
        // runs always have one thanks to the non-GitHub-remote guard, but
        // unit-test contexts via `CompileContext::for_test` will not.
        let org = ctx
            .ado_org()
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();
        let repo = ctx
            .ado_context
            .as_ref()
            .map(|c| c.repo_name.to_ascii_lowercase())
            .unwrap_or_default();

        let metadata_json = serde_json::json!({
            "schema": 1,
            "source": source,
            "org": org,
            "repo": repo,
            "version": version,
            "target": target,
        })
        .to_string();

        // The `# ado-aw-metadata:` line is the parse target for
        // discovery. The `echo` makes the same information visible in
        // the build log at runtime, which is a free human-discoverability
        // bonus and costs nothing because the step runs in milliseconds.
        //
        // The echo's user-controlled values go through two sanitisations:
        //
        //  1. `crate::sanitize::neutralize_pipeline_commands` neutralises
        //     `##vso[` and `##[` prefixes by wrapping them in backticks.
        //     The ADO build agent scans stdout for those sequences and
        //     treats them as logging commands (e.g. `task.setvariable`).
        //     An attacker who controls a markdown filename could
        //     otherwise inject a logging command into the build log via
        //     the echoed source path. Reusing the canonical helper keeps
        //     this in sync with the rest of the sanitisation surfaces.
        //
        //  2. `bash_single_quote_escape` applies the `'\''` idiom so a
        //     filename containing `'` (e.g. `agents/foo's.md`) doesn't
        //     produce syntactically broken bash. `version` and `target`
        //     are controlled inputs and can't contain either.
        //
        // `org` and `repo` are derived from ADO remote parsing, which
        // already restricts them to a safe character set, but we apply
        // the same defence-in-depth pattern for consistency.
        let echo_source = bash_single_quote_escape(
            &crate::sanitize::neutralize_pipeline_commands(&source),
        );
        let echo_org = bash_single_quote_escape(
            &crate::sanitize::neutralize_pipeline_commands(&org),
        );
        let echo_repo = bash_single_quote_escape(
            &crate::sanitize::neutralize_pipeline_commands(&repo),
        );
        let step = format!(
            "- bash: |\n    \
                # ado-aw-metadata: {metadata}\n    \
                echo 'ado-aw metadata: source={echo_source} org={echo_org} repo={echo_repo} version={version} target={target}'\n  \
            displayName: \"ado-aw\"\n",
            metadata = metadata_json,
            echo_source = echo_source,
            echo_org = echo_org,
            echo_repo = echo_repo,
            version = version,
            target = target,
        );

        vec![step]
    }
}

/// Escape any `'` in `s` so it can be safely embedded inside a single-quoted
/// bash string. Replaces each `'` with `'\''` (close-quote, escaped quote,
/// reopen-quote — the canonical idiom).
fn bash_single_quote_escape(s: &str) -> String {
    s.replace('\'', "'\\''")
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
        let steps = AdoAwMarkerExtension.prepare_steps(&ctx);
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
        let steps = AdoAwMarkerExtension.prepare_steps(&ctx);
        assert_eq!(steps.len(), 1);
        let step = &steps[0];
        assert!(step.contains("displayName: \"ado-aw\""), "step missing displayName:\n{step}");
        assert!(step.contains("# ado-aw-metadata:"), "step missing JSON marker line:\n{step}");
        assert!(step.contains("\"source\":\"agents/foo.md\""), "step missing source field:\n{step}");
        assert!(step.contains("\"target\":\"standalone\""), "step missing target field:\n{step}");
        assert!(step.contains("\"schema\":1"), "step missing schema field:\n{step}");
        // No ado_context => org/repo emit as empty strings.
        assert!(step.contains("\"org\":\"\""), "step missing org field:\n{step}");
        assert!(step.contains("\"repo\":\"\""), "step missing repo field:\n{step}");
    }

    #[test]
    fn org_and_repo_embed_from_ado_context_lowercased() {
        // When the compiler runs inside an ADO checkout (the production
        // path — the non-GitHub-remote guard enforces this), the JSON
        // marker carries `org` and `repo` so discovery can disambiguate
        // a same-named `source` across two repos in the same project.
        let fm = parse_fm("name: t\ndescription: x\n");
        let input_path = Path::new("agents/foo.md");
        let ctx = CompileContext {
            agent_name: &fm.name,
            front_matter: &fm,
            ado_context: Some(crate::ado::AdoContext {
                org_url: "https://dev.azure.com/MyOrg".to_string(),
                project: "MyProject".to_string(),
                repo_name: "Templates-A".to_string(),
            }),
            engine: crate::engine::Engine::Copilot,
            compile_dir: None,
            input_path: Some(input_path),
        };
        let steps = AdoAwMarkerExtension.prepare_steps(&ctx);
        assert_eq!(steps.len(), 1);
        let step = &steps[0];
        // ADO identifiers are case-insensitive; lowercase to make
        // comparisons in discovery deterministic.
        assert!(
            step.contains("\"org\":\"myorg\""),
            "expected lowercased org field:\n{step}"
        );
        assert!(
            step.contains("\"repo\":\"templates-a\""),
            "expected lowercased repo field:\n{step}"
        );
        // The echo line surfaces them too for build-log readability.
        assert!(
            step.contains("org=myorg repo=templates-a"),
            "expected echo to include org/repo:\n{step}"
        );
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
            let steps = AdoAwMarkerExtension.prepare_steps(&ctx);
            assert_eq!(steps.len(), 1, "target={raw_target}");
            assert!(
                steps[0].contains(&format!("\"target\":\"{expected}\"")),
                "expected target={expected} in step (raw input {raw_target}):\n{}",
                steps[0]
            );
        }
    }

    #[test]
    fn bash_single_quote_escape_idiom_is_correct() {
        // Standard bash idiom: close-quote, escaped quote, reopen.
        assert_eq!(bash_single_quote_escape("a'b"), "a'\\''b");
        assert_eq!(bash_single_quote_escape("''"), "'\\'''\\''");
        assert_eq!(bash_single_quote_escape("plain"), "plain");
        assert_eq!(bash_single_quote_escape(""), "");
    }

    #[test]
    fn echo_line_handles_single_quote_in_source_path() {
        // A markdown filename with `'` in it must produce syntactically
        // valid bash. Without the escape, the generated step would
        // break with "unexpected EOF while looking for matching `''".
        let fm = parse_fm("name: t\ndescription: x\n");
        let input_path = Path::new("agents/foo's-agent.md");
        let ctx = CompileContext {
            agent_name: &fm.name,
            front_matter: &fm,
            ado_context: None,
            engine: crate::engine::Engine::Copilot,
            compile_dir: None,
            input_path: Some(input_path),
        };
        let steps = AdoAwMarkerExtension.prepare_steps(&ctx);
        assert_eq!(steps.len(), 1);
        let step = &steps[0];
        assert!(
            step.contains("echo 'ado-aw metadata: source=agents/foo'\\''s-agent.md "),
            "single-quote in source should be escaped via the '\\'' idiom; got:\n{step}",
        );
        // The JSON marker line should still carry the raw (un-bash-escaped)
        // source — JSON has no quoting concern with `'`.
        assert!(
            step.contains("\"source\":\"agents/foo's-agent.md\""),
            "JSON marker should carry raw source unchanged:\n{step}",
        );
    }

    #[test]
    fn echo_line_neutralises_vso_injection_attempt() {
        // An attacker who controls a markdown filename must not be able
        // to inject ADO logging commands into the build log via the
        // echoed source path. The ADO agent scans stdout for `##vso[`
        // and `##[` prefixes and treats matching sequences as task
        // commands (setvariable, setoutput, etc.).
        //
        // Marker uses the canonical `crate::sanitize::neutralize_pipeline_commands`
        // which backtick-wraps the prefix (`` `##vso[` ``) — the literal
        // `##vso[` no longer starts a token in the agent's scanner. See
        // `src/sanitize.rs` for the canonical helper's own tests.
        let fm = parse_fm("name: t\ndescription: x\n");
        let input_path = Path::new("agents/##vso[task.setvariable variable=FOO]value.md");
        let ctx = CompileContext {
            agent_name: &fm.name,
            front_matter: &fm,
            ado_context: None,
            engine: crate::engine::Engine::Copilot,
            compile_dir: None,
            input_path: Some(input_path),
        };
        let steps = AdoAwMarkerExtension.prepare_steps(&ctx);
        assert_eq!(steps.len(), 1);
        let step = &steps[0];

        // Find the `echo` line specifically — the `# ado-aw-metadata`
        // JSON line is allowed to carry the raw source (it's not echoed
        // to stdout by ADO; it's a comment in the bash heredoc, not
        // output at runtime). The JSON line *does* get written to the
        // build log when ADO renders the step body, but as `# ...`
        // comments inside the rendered yaml; those don't trip the
        // logging-command scanner.
        let echo_line = step
            .lines()
            .find(|l| l.trim_start().starts_with("echo 'ado-aw metadata:"))
            .expect("must have echo line");
        // `neutralize_pipeline_commands` wraps the matched prefix in
        // backticks, breaking the `##vso[` token at the start of the
        // sequence. The agent's scanner is anchored to the literal
        // prefix; the backtick-wrapped form passes through unprocessed.
        assert!(
            !echo_line.contains(" ##vso["),
            "raw ##vso[ leaked into echo line (must be backtick-wrapped): {echo_line}"
        );
        assert!(
            echo_line.contains("`##vso[`"),
            "expected `##vso[` neutralised via canonical backtick-wrap in echo line: {echo_line}"
        );
    }

    #[test]
    fn json_marker_quote_in_source_round_trips_correctly() {
        // Regression: `normalize_source_path` previously escaped `"` to
        // `\"` before embedding the path. `serde_json::json!` then
        // double-encoded the backslash, so the marker JSON looked like
        // `"source":"agents/foo\\\"bar.md"` — and the path returned by
        // `parse_marker_step` carried a spurious `\` that did not exist
        // in the original filename. The fix is to feed the canonical
        // (unescaped) path into the JSON value and let serde_json do
        // the JSON-level escaping.
        let fm = parse_fm("name: t\ndescription: x\n");
        let input_path = Path::new(r#"agents/foo"bar.md"#);
        let ctx = CompileContext {
            agent_name: &fm.name,
            front_matter: &fm,
            ado_context: None,
            engine: crate::engine::Engine::Copilot,
            compile_dir: None,
            input_path: Some(input_path),
        };
        let steps = AdoAwMarkerExtension.prepare_steps(&ctx);
        assert_eq!(steps.len(), 1);

        // Parse the marker step back via the canonical discovery parser
        // and confirm the source field reconstructs to the original
        // path (forward-slash-normalised, no spurious backslashes).
        let parsed = crate::detect::parse_marker_step(&steps[0]);
        assert_eq!(parsed.len(), 1, "expected exactly one marker in step");
        assert_eq!(
            parsed[0].source,
            r#"agents/foo"bar.md"#,
            "marker source should round-trip without spurious backslash"
        );
    }
}