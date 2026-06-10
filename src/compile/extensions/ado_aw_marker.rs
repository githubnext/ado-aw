//! Always-on ado-aw marker extension.
//!
//! Injects two informational steps into the prepare phase of the
//! Agent job of every compiled pipeline. One step carries the existing
//! `# ado-aw-metadata:` discovery marker, and the other writes a
//! machine-readable `staging/aw_info.json` runtime artifact for audit.
//!
//! Why `prepare_steps` (Agent job) and not `setup_steps` (Setup job):
//! a Setup-job injection would force every compiled pipeline to spin
//! up a dedicated pool agent just to emit a metadata comment, even for
//! pipelines that have no other reason to need a Setup job. The Agent
//! job is always present, so `prepare_steps` is free.
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

use super::{CompileContext, CompilerExtension, Declarations, ExtensionPhase};
use crate::compile::ir::condition::Condition;
use crate::compile::ir::step::{BashStep, Step};

// ─── ado-aw marker (always-on, internal) ─────────────────────────────

/// Always-on internal extension that embeds machine-readable
/// `# ado-aw-metadata: {…}` JSON inside an injected Agent-job prepare
/// step and writes a matching `staging/aw_info.json` artifact.
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
        // Inject the marker steps into the Agent job's prepare phase
        // (NOT a separate Setup job). Setup-job injection would force
        // every compiled pipeline to spin up an extra agent pool job
        // just to emit metadata — wasteful for pipelines that have no
        // other reason to need a Setup job. prepare_steps lands inside
        // the always-present Agent job's `{{ prepare_steps }}` block,
        // so it costs zero extra jobs/agents/pool time.
        let Some(metadata) = CompileMetadata::from_ctx(ctx) else {
            return vec![];
        };

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
        //  2. `bash_single_quote_escape` applies the `\''` idiom so a
        //     filename containing `'` (e.g. `agents/foo's.md`) doesn't
        //     produce syntactically broken bash. `version` and `target`
        //     are controlled inputs and can't contain either.
        //
        // `org` and `repo` are derived from ADO remote parsing, which
        // already restricts them to a safe character set, but we apply
        // the same defence-in-depth pattern for consistency.
        let echo_source = bash_single_quote_escape(&crate::sanitize::neutralize_pipeline_commands(
            &metadata.source,
        ));
        let echo_org = bash_single_quote_escape(&crate::sanitize::neutralize_pipeline_commands(
            &metadata.org,
        ));
        let echo_repo = bash_single_quote_escape(&crate::sanitize::neutralize_pipeline_commands(
            &metadata.repo,
        ));

        let marker_step = format!(
            r#"- bash: |
    # ado-aw-metadata: {metadata_json}
    echo 'ado-aw metadata: source={echo_source} org={echo_org} repo={echo_repo} version={version} target={target}'
  displayName: "ado-aw"
"#,
            metadata_json = metadata.marker_json(),
            echo_source = echo_source,
            echo_org = echo_org,
            echo_repo = echo_repo,
            version = metadata.compiler_version.as_str(),
            target = metadata.target.as_str(),
        );

        let aw_info_step = format!(
            r#"- bash: |
    set -eo pipefail

    mkdir -p "$(Agent.TempDirectory)/staging"
    cat >"$(Agent.TempDirectory)/staging/aw_info.json" <<'AW_INFO_EOF'
    {aw_info_json}
    AW_INFO_EOF
  displayName: "Emit aw_info.json"
  condition: always()
"#,
            aw_info_json = metadata.aw_info_json(),
        );

        vec![marker_step, aw_info_step]
    }

    /// Typed-IR view of the two prepare steps emitted by
    /// [`Self::prepare_steps`]. Returns the same two bash steps as
    /// `Step::Bash(BashStep)` values — the bash bodies are
    /// byte-identical so lowering through `ir::emit` produces the
    /// same YAML as today.
    ///
    /// Coexists with `prepare_steps` until the
    /// `compile-target-standalone` commit switches production
    /// consumption to `declarations`.
    fn declarations(&self, ctx: &CompileContext) -> anyhow::Result<Declarations> {
        let Some(metadata) = CompileMetadata::from_ctx(ctx) else {
            return Ok(Declarations::default());
        };
        let agent_prepare_steps = vec![
            Step::Bash(marker_bash_step(&metadata)),
            Step::Bash(aw_info_bash_step(&metadata)),
        ];
        Ok(Declarations {
            agent_prepare_steps,
            ..Declarations::default()
        })
    }
}

/// Build the typed [`BashStep`] form of the `# ado-aw-metadata: …`
/// marker step. The script body is byte-identical to the YAML
/// embedded by [`AdoAwMarkerExtension::prepare_steps`] so the two
/// emission paths produce equivalent pipelines.
fn marker_bash_step(metadata: &CompileMetadata) -> BashStep {
    let echo_source = bash_single_quote_escape(&crate::sanitize::neutralize_pipeline_commands(
        &metadata.source,
    ));
    let echo_org = bash_single_quote_escape(&crate::sanitize::neutralize_pipeline_commands(
        &metadata.org,
    ));
    let echo_repo = bash_single_quote_escape(&crate::sanitize::neutralize_pipeline_commands(
        &metadata.repo,
    ));
    let script = format!(
        "# ado-aw-metadata: {metadata_json}\n\
         echo 'ado-aw metadata: source={echo_source} org={echo_org} repo={echo_repo} version={version} target={target}'\n",
        metadata_json = metadata.marker_json(),
        echo_source = echo_source,
        echo_org = echo_org,
        echo_repo = echo_repo,
        version = metadata.compiler_version.as_str(),
        target = metadata.target.as_str(),
    );
    BashStep::new("ado-aw", script)
}

/// Build the typed [`BashStep`] form of the `aw_info.json` emit step.
fn aw_info_bash_step(metadata: &CompileMetadata) -> BashStep {
    let script = format!(
        "set -eo pipefail\n\
         \n\
         mkdir -p \"$(Agent.TempDirectory)/staging\"\n\
         cat >\"$(Agent.TempDirectory)/staging/aw_info.json\" <<'AW_INFO_EOF'\n\
         {aw_info_json}\n\
         AW_INFO_EOF\n",
        aw_info_json = metadata.aw_info_json(),
    );
    BashStep::new("Emit aw_info.json", script).with_condition(Condition::Always)
}

struct CompileMetadata {
    source: String,
    org: String,
    repo: String,
    compiler_version: String,
    target: String,
    engine: String,
    model: String,
    agent_name: String,
}

impl CompileMetadata {
    fn from_ctx(ctx: &CompileContext) -> Option<Self> {
        let input_path = ctx.input_path?;
        Some(Self {
            source: super::super::common::normalize_source_path(input_path),
            org: ctx
                .ado_org()
                .map(|s| s.to_ascii_lowercase())
                .unwrap_or_default(),
            repo: ctx
                .ado_context
                .as_ref()
                .map(|c| c.repo_name.to_ascii_lowercase())
                .unwrap_or_default(),
            compiler_version: env!("CARGO_PKG_VERSION").to_string(),
            target: ctx.front_matter.target.as_str().to_string(),
            engine: ctx.front_matter.engine.engine_id().to_string(),
            model: match ctx.engine {
                crate::engine::Engine::Copilot => ctx
                    .front_matter
                    .engine
                    .model()
                    .unwrap_or(crate::engine::DEFAULT_COPILOT_MODEL)
                    .to_string(),
            },
            agent_name: ctx.agent_name.to_string(),
        })
    }

    fn marker_json(&self) -> String {
        serde_json::to_string(&serde_json::json!({
            "schema": 1,
            "source": &self.source,
            "org": &self.org,
            "repo": &self.repo,
            "version": &self.compiler_version,
            "target": &self.target,
        }))
        .unwrap()
    }

    fn aw_info_json(&self) -> String {
        serde_json::to_string(&serde_json::json!({
            "schema": "ado-aw/aw_info/1",
            "source": &self.source,
            "org": &self.org,
            "repo": &self.repo,
            "compiler_version": &self.compiler_version,
            "target": &self.target,
            "engine": &self.engine,
            "model": &self.model,
            "agent_name": &self.agent_name,
            "build_id": "$(Build.BuildId)",
            "source_version": "$(Build.SourceVersion)",
            "source_branch": "$(Build.SourceBranch)",
            "build_definition_id": "$(System.DefinitionId)",
        }))
        .unwrap()
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
        assert!(
            steps.is_empty(),
            "expected no marker when input_path is None"
        );
    }

    #[test]
    fn emits_marker_step_with_canonical_displayname() {
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
        assert_eq!(steps.len(), 2);
        let step = &steps[0];
        assert!(
            step.contains("displayName: \"ado-aw\""),
            "step missing displayName:\n{step}"
        );
        assert!(
            step.contains("# ado-aw-metadata:"),
            "step missing JSON marker line:\n{step}"
        );
        assert!(
            step.contains("\"source\":\"agents/foo.md\""),
            "step missing source field:\n{step}"
        );
        assert!(
            step.contains("\"target\":\"standalone\""),
            "step missing target field:\n{step}"
        );
        assert!(
            step.contains("\"schema\":1"),
            "step missing schema field:\n{step}"
        );
        // No ado_context => org/repo emit as empty strings.
        assert!(
            step.contains("\"org\":\"\""),
            "step missing org field:\n{step}"
        );
        assert!(
            step.contains("\"repo\":\"\""),
            "step missing repo field:\n{step}"
        );
    }

    #[test]
    fn emits_aw_info_step_with_expected_json_and_condition() {
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
        assert_eq!(steps.len(), 2);
        let step = &steps[1];
        assert!(
            step.contains("displayName: \"Emit aw_info.json\""),
            "step missing aw_info displayName:\n{step}"
        );
        assert!(
            step.contains("condition: always()"),
            "step missing always() condition:\n{step}"
        );
        assert!(
            step.contains("cat >\"$(Agent.TempDirectory)/staging/aw_info.json\" <<'AW_INFO_EOF'"),
            "step missing quoted heredoc write:\n{step}"
        );
        assert!(
            step.contains("\"schema\":\"ado-aw/aw_info/1\""),
            "step missing aw_info schema:\n{step}"
        );
        assert!(
            step.contains("\"source\":\"agents/foo.md\""),
            "step missing source field:\n{step}"
        );
        assert!(
            step.contains("\"target\":\"standalone\""),
            "step missing target field:\n{step}"
        );
        assert!(
            step.contains("\"engine\":\"copilot\""),
            "step missing engine field:\n{step}"
        );
        assert!(
            step.contains(&format!(
                "\"model\":\"{}\"",
                crate::engine::DEFAULT_COPILOT_MODEL
            )),
            "step missing default model field:\n{step}"
        );
        assert!(
            step.contains("\"agent_name\":\"t\""),
            "step missing agent_name field:\n{step}"
        );
        assert!(
            step.contains("\"build_id\":\"$(Build.BuildId)\""),
            "step missing build_id macro:\n{step}"
        );
        assert!(
            step.contains("\"source_version\":\"$(Build.SourceVersion)\""),
            "step missing source_version macro:\n{step}"
        );
        assert!(
            step.contains("\"source_branch\":\"$(Build.SourceBranch)\""),
            "step missing source_branch macro:\n{step}"
        );
        assert!(
            step.contains("\"build_definition_id\":\"$(System.DefinitionId)\""),
            "step missing build_definition_id macro:\n{step}"
        );
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
        assert_eq!(steps.len(), 2);
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
            assert_eq!(steps.len(), 2, "target={raw_target}");
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

    /// Typed-IR view of the same two steps. Locks the
    /// `declarations()` override against silent drift: must return
    /// exactly two `Step::Bash` values (no `Step::RawYaml` migration
    /// bridge) with the canonical display names. Detailed bash-body
    /// assertions still live in the legacy-form tests above; this
    /// test enforces shape, not content.
    #[test]
    fn declarations_returns_typed_bash_steps_not_raw_yaml() {
        use crate::compile::ir::step::Step;
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
        let decl = AdoAwMarkerExtension.declarations(&ctx).unwrap();
        assert_eq!(decl.agent_prepare_steps.len(), 2);
        match (&decl.agent_prepare_steps[0], &decl.agent_prepare_steps[1]) {
            (Step::Bash(marker), Step::Bash(aw_info)) => {
                assert_eq!(marker.display_name, "ado-aw");
                assert!(marker.script.contains("# ado-aw-metadata:"));
                assert_eq!(aw_info.display_name, "Emit aw_info.json");
                assert!(matches!(
                    aw_info.condition,
                    Some(crate::compile::ir::condition::Condition::Always)
                ));
            }
            (a, b) => panic!("expected (Step::Bash, Step::Bash), got ({a:?}, {b:?})"),
        }
        // All other Declarations slots must be empty - the marker
        // extension contributes nothing else.
        assert!(decl.setup_steps.is_empty());
        assert!(decl.network_hosts.is_empty());
        assert!(decl.mcpg_servers.is_empty());
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
        assert_eq!(steps.len(), 2);
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
        assert_eq!(steps.len(), 2);
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
        assert_eq!(steps.len(), 2);

        // Parse the marker step back via the canonical discovery parser
        // and confirm the source field reconstructs to the original
        // path (forward-slash-normalised, no spurious backslashes).
        let parsed = crate::detect::parse_marker_step(&steps[0]);
        assert_eq!(parsed.len(), 1, "expected exactly one marker in step");
        assert_eq!(
            parsed[0].source, r#"agents/foo"bar.md"#,
            "marker source should round-trip without spurious backslash"
        );
    }
}
