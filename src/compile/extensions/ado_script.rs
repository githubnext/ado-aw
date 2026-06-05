//! Single always-on extension that delivers and runs ado-script bundles
//! (gate.js, import.js) inside the ADO pipeline.
//!
//! Two features, each emitted into the job that actually consumes the
//! bundle:
//!
//! - **Gate evaluator (`gate.js`)** — runs in the **Setup job** when
//!   `filters:` lowers to non-empty checks. Emitted via
//!   [`AdoScriptExtension::setup_steps`].
//! - **Runtime-import resolver (`import.js`)** — runs in the **Agent
//!   job** when `inlined-imports: false`. Emitted via
//!   [`AdoScriptExtension::prepare_steps`], which the compiler lands
//!   in the existing `{{ prepare_steps }}` block.
//!
//! ## Why per-job emission
//!
//! ADO jobs use isolated VMs — `/tmp` is **not** shared. The
//! `ado-script.zip` bundle is therefore downloaded once per consuming
//! job. When both features are active, install + download steps appear
//! in **both** Setup and Agent. That's correct architecture given ADO's
//! topology, not waste.

use anyhow::Result;

use super::{CompileContext, CompilerExtension, ExtensionPhase};
use crate::compile::filter_ir::{
    GateContext, Severity, compile_gate_step_external, lower_pipeline_filters, lower_pr_filters,
    validate_pipeline_filters, validate_pr_filters,
};
use crate::compile::types::{PipelineFilters, PrFilters};

const GATE_EVAL_PATH: &str = "/tmp/ado-aw-scripts/ado-script/gate.js";
pub(crate) const IMPORT_EVAL_PATH: &str = "/tmp/ado-aw-scripts/ado-script/import.js";
/// Path to the exec-context-pr bundle inside the unpacked `ado-script.zip`.
/// Consumed by `src/compile/extensions/exec_context/pr.rs` to invoke
/// the bundle from the PR contributor's prepare step.
pub const EXEC_CONTEXT_PR_PATH: &str = "/tmp/ado-aw-scripts/ado-script/exec-context-pr.js";
const RELEASE_BASE_URL: &str = "https://github.com/githubnext/ado-aw/releases/download";

/// Single always-on extension that owns all `ado-script` bundle wiring.
pub struct AdoScriptExtension {
    pub pr_filters: Option<PrFilters>,
    pub pipeline_filters: Option<PipelineFilters>,
    pub inlined_imports: bool,
    /// Whether the PR-context contributor will activate. When true,
    /// the Agent-job install/download must fire even if
    /// `runtime_imports_active()` is false (i.e. the user has
    /// `inlined-imports: true` but a PR trigger configured), so that
    /// `exec-context-pr.js` is present for the `pr.rs` invocation.
    ///
    /// Populated at construction by `collect_extensions` using the
    /// shared `exec_context_pr_active` predicate so this stays in
    /// lock-step with `ExecContextExtension`'s own activation gate.
    pub exec_context_pr_active: bool,
}

impl AdoScriptExtension {
    /// Compute the lowered PR and pipeline checks once. Returns
    /// `(pr_checks, pipeline_checks)`; either may be empty, in which
    /// case the corresponding gate step is not emitted.
    ///
    /// Lowering is cheap but `setup_steps()` and `has_gate()`-style
    /// callers used to invoke `lower_*_filters()` twice (once to test
    /// emptiness, once to materialize). This helper folds both passes
    /// into a single computation that callers reuse.
    fn lowered_checks(
        &self,
    ) -> (
        Vec<crate::compile::filter_ir::FilterCheck>,
        Vec<crate::compile::filter_ir::FilterCheck>,
    ) {
        let pr_checks = self
            .pr_filters
            .as_ref()
            .map(lower_pr_filters)
            .unwrap_or_default();
        let pipeline_checks = self
            .pipeline_filters
            .as_ref()
            .map(lower_pipeline_filters)
            .unwrap_or_default();
        (pr_checks, pipeline_checks)
    }

    fn has_gate(&self) -> bool {
        let (pr, pipeline) = self.lowered_checks();
        !pr.is_empty() || !pipeline.is_empty()
    }

    fn runtime_imports_active(&self) -> bool {
        !self.inlined_imports
    }
}

/// Returns the two-step bundle: NodeTool@0 install + checksumed unzip of
/// `ado-script.zip`. Shared between [`AdoScriptExtension::setup_steps`]
/// and [`AdoScriptExtension::prepare_steps`] — emitted twice in the YAML
/// when both consumers are active, once per consuming job's VM.
fn install_and_download_steps() -> Vec<String> {
    let version = env!("CARGO_PKG_VERSION");
    vec![
        // NodeTool@0 — install Node 20.x. Pinned LTS major; any patch
        // release is fine for this use. The display name no longer
        // mentions the gate evaluator because import.js uses Node too.
        // A 5-minute timeout caps the worst-case cold-image install.
        r#"- task: NodeTool@0
  inputs:
    versionSpec: "20.x"
  displayName: "Install Node.js 20.x"
  timeoutInMinutes: 5
  condition: succeeded()"#
            .to_string(),
        // curl + sha256 + unzip pipeline. Same 5-minute bound so a
        // stalled CDN response doesn't tie up the whole pipeline. The
        // explicit `-d` on unzip is belt-and-suspenders zip-slip
        // hardening on top of the sha256 verification.
        format!(
            r#"- bash: |
    set -eo pipefail
    mkdir -p /tmp/ado-aw-scripts
    curl -fsSL "{RELEASE_BASE_URL}/v{version}/checksums.txt" -o /tmp/ado-aw-scripts/checksums.txt
    curl -fsSL "{RELEASE_BASE_URL}/v{version}/ado-script.zip" -o /tmp/ado-aw-scripts/ado-script.zip
    cd /tmp/ado-aw-scripts && grep "ado-script.zip" checksums.txt | sha256sum -c -
    unzip -o /tmp/ado-aw-scripts/ado-script.zip -d /tmp/ado-aw-scripts/
  displayName: "Download ado-aw scripts (v{version})"
  timeoutInMinutes: 5
  condition: succeeded()"#,
        ),
    ]
}

/// The resolver step that runs in the Agent job to expand
/// `{{#runtime-import …}}` markers in the agent prompt file in place.
///
/// Passes `--base "$(Build.SourcesDirectory)"` so that `import.js`
/// resolves the compiler-emitted trigger-repo-relative marker against
/// the trigger-repo checkout root. `import.js` rejects absolute marker
/// paths (matching the compile-time `resolve_imports_inline` policy)
/// so the relative-form marker is the only form that ever needs to
/// resolve at runtime.
fn resolver_step() -> String {
    format!(
        r#"- bash: |
    set -eo pipefail
    node '{IMPORT_EVAL_PATH}' /tmp/awf-tools/agent-prompt.md --base "$(Build.SourcesDirectory)"
  displayName: "Resolve runtime imports (agent prompt)"
  condition: succeeded()"#
    )
}

impl CompilerExtension for AdoScriptExtension {
    fn name(&self) -> &str {
        "ado-script"
    }

    fn phase(&self) -> ExtensionPhase {
        // System phase: ado-script's NodeTool@0 install + bundle download +
        // resolver step must complete BEFORE any user-facing Runtime
        // extension (e.g. NodeExtension) runs. Otherwise our Node 20
        // install would prepend onto PATH after the user's pinned Node,
        // silently overriding the user's choice for the rest of the
        // Agent job. By running first, our install lives only during the
        // brief window before the user's Runtime install, and the
        // resolver step inside that window picks up our Node 20.
        ExtensionPhase::System
    }

    fn setup_steps(&self, _ctx: &CompileContext) -> Result<Vec<String>> {
        let (pr_checks, pipeline_checks) = self.lowered_checks();
        if pr_checks.is_empty() && pipeline_checks.is_empty() {
            return Ok(vec![]);
        }
        let mut steps = install_and_download_steps();
        if !pr_checks.is_empty() {
            steps.push(compile_gate_step_external(
                GateContext::PullRequest,
                &pr_checks,
                GATE_EVAL_PATH,
            )?);
        }
        if !pipeline_checks.is_empty() {
            steps.push(compile_gate_step_external(
                GateContext::PipelineCompletion,
                &pipeline_checks,
                GATE_EVAL_PATH,
            )?);
        }
        Ok(steps)
    }

    fn prepare_steps(&self, _ctx: &CompileContext) -> Vec<String> {
        // The Agent-job install/download must fire when ANY downstream
        // consumer is active. Today there are two:
        //  - `import.js` (runtime-import resolver) — runs when
        //    `inlined-imports: false`.
        //  - `exec-context-pr.js` (PR-context precompute) — runs when
        //    the PR contributor activates (`on.pr` configured AND
        //    `execution-context.pr.enabled != false`).
        //
        // The exec-context-pr invocation itself is emitted by
        // `ExecContextExtension::prepare_steps` (Tool phase, runs
        // after this System-phase install/download), not here, so the
        // two extensions stay loosely coupled.
        let import_active = self.runtime_imports_active();
        if !import_active && !self.exec_context_pr_active {
            return vec![];
        }
        let mut steps = install_and_download_steps();
        if import_active {
            steps.push(resolver_step());
        }
        steps
    }

    fn validate(&self, _ctx: &CompileContext) -> Result<Vec<String>> {
        let mut warnings = Vec::new();
        if let Some(f) = &self.pr_filters {
            for diag in validate_pr_filters(f) {
                match diag.severity {
                    Severity::Error => anyhow::bail!("{}", diag),
                    Severity::Warning | Severity::Info => {
                        warnings.push(format!("{}", diag));
                    }
                }
            }
        }
        if let Some(f) = &self.pipeline_filters {
            for diag in validate_pipeline_filters(f) {
                match diag.severity {
                    Severity::Error => anyhow::bail!("{}", diag),
                    Severity::Warning | Severity::Info => {
                        warnings.push(format!("{}", diag));
                    }
                }
            }
        }
        Ok(warnings)
    }

    fn required_hosts(&self) -> Vec<String> {
        // Only request github.com when the bundle is actually downloaded.
        // When `inlined-imports: true` AND no filters are configured,
        // neither `setup_steps()` nor `prepare_steps()` emits the
        // NodeTool@0 + curl pair, so the github.com release-asset host
        // is never reached and shouldn't be on the allowlist. The host
        // list is allowlist-additive across extensions, so this stays
        // safe even when other extensions independently need github.com.
        if self.has_gate() || self.runtime_imports_active() {
            vec!["github.com".to_string()]
        } else {
            vec![]
        }
    }
}

/// Resolve `{{#runtime-import path}}` markers in `body` at compile time.
///
/// Used by `compile_shared` when `inlined-imports: true` so author-written
/// markers inside the agent's markdown body still work in inlined mode.
///
/// Path resolution: only **relative** paths are accepted. They are
/// resolved against `base_dir` (the source `.md` file's directory).
/// Absolute paths and `..` segments are rejected because compile-time
/// resolution against an untrusted branch (e.g. `ado-aw compile` on a
/// hostile PR) would otherwise embed arbitrary host files
/// (`{{#runtime-import /home/runner/.ssh/id_rsa}}`,
/// `{{#runtime-import ../../../../etc/passwd}}`) verbatim into the
/// compiled YAML.
///
/// Required markers fail with an error; optional `?`-form markers
/// silently drop if the referenced file is missing.
pub fn resolve_imports_inline(body: &str, base_dir: &std::path::Path) -> Result<String> {
    const MARKER_PREFIX: &str = "{{#runtime-import";

    let mut result = String::with_capacity(body.len());
    let mut cursor = 0usize;

    while let Some(rel_start) = body[cursor..].find(MARKER_PREFIX) {
        let start = cursor + rel_start;
        result.push_str(&body[cursor..start]);

        let marker_body_start = start + MARKER_PREFIX.len();
        let rel_end = body[marker_body_start..].find("}}").ok_or_else(|| {
            anyhow::anyhow!(
                "runtime-import: unterminated marker starting at byte {}",
                start
            )
        })?;
        let marker_end = marker_body_start + rel_end;
        let marker_body = body[marker_body_start..marker_end].trim();

        let (optional, path_str) = if let Some(rest) = marker_body.strip_prefix('?') {
            (true, rest.trim())
        } else {
            (false, marker_body)
        };

        anyhow::ensure!(
            !path_str.is_empty(),
            "runtime-import: missing path in marker '{}'",
            &body[start..marker_end + 2]
        );
        anyhow::ensure!(
            !path_str.chars().any(char::is_whitespace),
            "runtime-import: invalid path '{}': whitespace is not allowed",
            path_str
        );
        // Reject `}` in paths so the compile-time resolver stays in
        // strict parity with the runtime regex
        // (`scripts/ado-script/src/import/index.ts` — `[^\s}]+`). The
        // runtime regex stops the path capture at any `}`; the
        // compile-time resolver, by contrast, terminates only at the
        // closing `}}` and would otherwise happily accept a path like
        // `foo}bar.md`. Allowing `}` here would silently produce
        // different behaviour on the two paths (compile-time: file
        // looked up as `foo}bar.md`; runtime: marker survives
        // unexpanded). Reject up front so the failure mode is one
        // clear compile-time error in both modes.
        anyhow::ensure!(
            !path_str.contains('}'),
            "runtime-import: invalid path '{}': '}}' is not allowed (incompatible with the runtime resolver's path regex)",
            path_str
        );
        // Reject any path whose segments contain `..`. A malicious agent
        // body could otherwise reach files outside `base_dir` and embed
        // them verbatim into the compiled YAML — e.g.
        // `{{#runtime-import ../../../../etc/passwd}}` if `ado-aw compile`
        // is run on an untrusted PR branch. This guard applies to both
        // relative and absolute paths because `..` segments make any
        // path-confinement check unsound.
        anyhow::ensure!(
            !path_str
                .split(['/', '\\'])
                .any(|component| component == ".."),
            "runtime-import: invalid path '{}': '..' path components are not allowed",
            path_str
        );
        // Reject absolute paths at compile time. An untrusted PR branch
        // could otherwise embed arbitrary host files into the compiled
        // YAML (e.g. `{{#runtime-import /home/runner/.ssh/id_rsa}}`,
        // `{{#runtime-import C:\Users\…\secrets.txt}}`). Only relative
        // imports rooted in `base_dir` (the source `.md` file's
        // directory, which is part of the same repo) are safe.
        //
        // `Path::is_absolute` is platform-dependent: on Linux it
        // doesn't recognize `C:\foo` as absolute, and on Windows it
        // doesn't recognize a POSIX-style `/foo` UNC path. To make the
        // guard equally strict on every host where `ado-aw compile`
        // runs, also explicitly detect:
        //   - POSIX absolute (`/foo`)
        //   - Windows drive-letter absolute (`C:\foo`, `C:/foo`, any letter)
        //   - UNC (`\\server\share`)
        let is_drive_letter_absolute = {
            let mut chars = path_str.chars();
            matches!(
                (chars.next(), chars.next(), chars.next()),
                (Some(c), Some(':'), Some(sep))
                    if c.is_ascii_alphabetic() && (sep == '/' || sep == '\\')
            )
        };
        let is_absolute = std::path::Path::new(path_str).is_absolute()
            || path_str.starts_with('/')
            || path_str.starts_with("\\\\")
            || is_drive_letter_absolute;
        anyhow::ensure!(
            !is_absolute,
            "runtime-import: invalid path '{}': absolute paths are not allowed (use a relative path rooted at the agent's directory)",
            path_str
        );

        let abs = base_dir.join(path_str);

        match std::fs::read_to_string(&abs) {
            Ok(contents) => result.push_str(&contents),
            Err(_) if optional => {}
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "runtime-import: file not found: {} ({})",
                    path_str,
                    e
                ));
            }
        }

        cursor = marker_end + 2;
    }

    result.push_str(&body[cursor..]);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::extensions::CompileContext;
    use crate::compile::types::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ── extension behaviour ────────────────────────────────────────────

    fn ext_with(
        pr: Option<PrFilters>,
        pipeline: Option<PipelineFilters>,
        inlined: bool,
    ) -> AdoScriptExtension {
        AdoScriptExtension {
            pr_filters: pr,
            pipeline_filters: pipeline,
            inlined_imports: inlined,
            exec_context_pr_active: false,
        }
    }

    #[test]
    fn name_and_phase() {
        let ext = ext_with(None, None, true);
        assert_eq!(ext.name(), "ado-script");
        // System phase ensures NodeTool@0 install + bundle download +
        // resolver run BEFORE user-facing Runtime extensions (e.g. the
        // Node runtime), so the user's pinned Node version wins on PATH
        // for the rest of the Agent job.
        assert_eq!(ext.phase(), ExtensionPhase::System);
    }

    #[test]
    fn setup_steps_empty_without_gate() {
        let ext = ext_with(None, None, true);
        let fm: FrontMatter = serde_yaml::from_str("name: t\ndescription: t").unwrap();
        let ctx = CompileContext::for_test(&fm);
        assert!(ext.setup_steps(&ctx).unwrap().is_empty());
    }

    #[test]
    fn setup_steps_emits_install_download_and_gate_when_gate_active() {
        let filters = PrFilters {
            labels: Some(LabelFilter {
                any_of: vec!["run-agent".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let ext = ext_with(Some(filters), None, true);
        let fm: FrontMatter = serde_yaml::from_str("name: t\ndescription: t").unwrap();
        let ctx = CompileContext::for_test(&fm);
        let steps = ext.setup_steps(&ctx).unwrap();
        assert_eq!(steps.len(), 3, "install + download + gate");
        assert!(steps[0].contains("NodeTool@0"));
        assert!(steps[0].contains("Install Node.js 20.x"));
        assert!(!steps[0].contains("for gate evaluator"));
        assert!(steps[1].contains("Download ado-aw scripts"));
        assert!(steps[1].contains("sha256sum -c -"));
        assert!(steps[2].contains("node '/tmp/ado-aw-scripts/ado-script/gate.js'"));
    }

    #[test]
    fn gate_and_import_eval_paths_consistent_with_download_step() {
        let extract_dir = "/tmp/ado-aw-scripts/";
        assert!(
            GATE_EVAL_PATH.starts_with(extract_dir),
            "GATE_EVAL_PATH must be under the unzip -d destination"
        );
        assert!(
            IMPORT_EVAL_PATH.starts_with(extract_dir),
            "IMPORT_EVAL_PATH must be under the unzip -d destination"
        );
        let zip_prefix = "ado-script/";
        assert!(
            GATE_EVAL_PATH
                .strip_prefix(extract_dir)
                .expect("gate path should include extract dir")
                .starts_with(zip_prefix),
            "GATE_EVAL_PATH suffix must match zip internal path prefix used in release.yml"
        );
        assert!(
            IMPORT_EVAL_PATH
                .strip_prefix(extract_dir)
                .expect("import path should include extract dir")
                .starts_with(zip_prefix),
            "IMPORT_EVAL_PATH suffix must match zip internal path prefix used in release.yml"
        );
        let steps = install_and_download_steps();
        let download = &steps[1];
        assert!(
            download.contains("-d /tmp/ado-aw-scripts/"),
            "download step must unzip to /tmp/ado-aw-scripts/"
        );
    }

    #[test]
    fn prepare_steps_empty_when_inlined_imports_true() {
        let ext = ext_with(None, None, true);
        let fm: FrontMatter = serde_yaml::from_str("name: t\ndescription: t").unwrap();
        let ctx = CompileContext::for_test(&fm);
        assert!(ext.prepare_steps(&ctx).is_empty());
    }

    #[test]
    fn prepare_steps_emits_install_download_and_resolver_when_runtime_imports_active() {
        let ext = ext_with(None, None, false);
        let fm: FrontMatter = serde_yaml::from_str("name: t\ndescription: t").unwrap();
        let ctx = CompileContext::for_test(&fm);
        let steps = ext.prepare_steps(&ctx);
        assert_eq!(steps.len(), 3, "install + download + resolver");
        assert!(steps[0].contains("NodeTool@0"));
        assert!(steps[1].contains("Download ado-aw scripts"));
        assert!(steps[2].contains("node '/tmp/ado-aw-scripts/ado-script/import.js'"));
        assert!(steps[2].contains("Resolve runtime imports (agent prompt)"));
        // The resolver receives `--base "$(Build.SourcesDirectory)"` so
        // the compiler-emitted trigger-repo-relative marker path
        // resolves correctly. Absolute paths in author markers are
        // rejected by import.js — see its absolute-path guard.
        assert!(
            steps[2].contains("--base \"$(Build.SourcesDirectory)\""),
            "resolver step must pass --base so trigger-repo-relative markers resolve correctly"
        );
        assert!(
            !steps[2].contains("ADO_AW_IMPORT_BASE"),
            "resolver step must not export ADO_AW_IMPORT_BASE — base is passed via --base, not env"
        );
    }

    #[test]
    fn validate_catches_min_gt_max_changes() {
        let filters = PrFilters {
            min_changes: Some(100),
            max_changes: Some(5),
            ..Default::default()
        };
        let ext = ext_with(Some(filters), None, true);
        let fm: FrontMatter = serde_yaml::from_str("name: t\ndescription: t").unwrap();
        let ctx = CompileContext::for_test(&fm);
        assert!(ext.validate(&ctx).is_err());
    }

    #[test]
    fn required_hosts_empty_when_no_consumer_active() {
        // inlined-imports: true AND no filters ⇒ no NodeTool / no
        // download / no gate / no resolver step. The github.com host
        // (used to fetch the release asset) is therefore unreachable
        // and must NOT be requested.
        let ext = ext_with(None, None, true);
        assert!(ext.required_hosts().is_empty());
    }

    #[test]
    fn required_hosts_requests_github_when_gate_active() {
        let filters = PrFilters {
            labels: Some(LabelFilter {
                any_of: vec!["run-agent".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let ext = ext_with(Some(filters), None, true);
        assert_eq!(ext.required_hosts(), vec!["github.com".to_string()]);
    }

    #[test]
    fn required_hosts_requests_github_when_runtime_imports_active() {
        // inlined-imports: false (default) ⇒ resolver step runs ⇒
        // github.com is needed for the bundle download.
        let ext = ext_with(None, None, false);
        assert_eq!(ext.required_hosts(), vec!["github.com".to_string()]);
    }

    // ── resolve_imports_inline ─────────────────────────────────────────

    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

    struct TestWorkspace {
        path: PathBuf,
    }

    impl TestWorkspace {
        fn new() -> Self {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::current_dir()
                .expect("current dir")
                .join("target")
                .join("ado-script-tests")
                .join(format!("{}-{}", std::process::id(), id));
            if path.exists() {
                let _ = fs::remove_dir_all(&path);
            }
            fs::create_dir_all(&path).expect("create workspace");
            Self { path }
        }

        fn write(&self, relative: &str, contents: &str) -> PathBuf {
            let path = self.path.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create parent");
            }
            fs::write(&path, contents).expect("write fixture file");
            path
        }
    }

    impl Drop for TestWorkspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn required_marker_resolves_to_file_contents() {
        let workspace = TestWorkspace::new();
        workspace.write("snippet.md", "hello from import\n");

        let result = resolve_imports_inline(
            "before\n{{#runtime-import snippet.md}}\nafter\n",
            &workspace.path,
        )
        .unwrap();

        assert_eq!(result, "before\nhello from import\n\nafter\n");
    }

    #[test]
    fn required_marker_missing_file_returns_error() {
        let workspace = TestWorkspace::new();
        let err =
            resolve_imports_inline("{{#runtime-import missing.md}}", &workspace.path).unwrap_err();
        assert!(
            err.to_string()
                .contains("runtime-import: file not found: missing.md")
        );
    }

    #[test]
    fn optional_marker_missing_file_replaces_with_empty_string() {
        let workspace = TestWorkspace::new();
        let result =
            resolve_imports_inline("pre{{#runtime-import? missing.md}}post", &workspace.path)
                .unwrap();
        assert_eq!(result, "prepost");
    }

    /// Relative paths under `base_dir` resolve correctly. Absolute paths
    /// are explicitly rejected — see `rejects_absolute_path_at_compile_time`.
    #[test]
    fn supports_relative_path_resolution() {
        let workspace = TestWorkspace::new();
        let nested_base = workspace.path.join("nested");
        fs::create_dir_all(&nested_base).unwrap();
        workspace.write("nested/relative.md", "relative-body");

        let relative =
            resolve_imports_inline("{{#runtime-import relative.md}}", &nested_base).unwrap();

        assert_eq!(relative, "relative-body");
    }

    /// Compile-time absolute-path rejection. The compile machine has
    /// privileged filesystem access (e.g. CI runners hold `.ssh/id_rsa`,
    /// hosted-pool service-connection material, dotfiles under the
    /// runner's home dir). An untrusted PR branch's markdown body must
    /// NOT be able to embed those files via
    /// `{{#runtime-import /home/runner/.ssh/id_rsa}}`. Only relative
    /// imports rooted under the agent's `.md` file's directory — which
    /// is itself inside the repo — are safe in adversarial scenarios.
    #[test]
    fn rejects_absolute_posix_path_at_compile_time() {
        let workspace = TestWorkspace::new();
        let err = resolve_imports_inline(
            "{{#runtime-import /etc/passwd}}",
            &workspace.path,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("absolute paths are not allowed"),
            "expected absolute-path rejection, got: {err}"
        );
    }

    #[test]
    fn rejects_absolute_windows_drive_path_at_compile_time() {
        let workspace = TestWorkspace::new();
        let err = resolve_imports_inline(
            r"{{#runtime-import C:\Users\runner\secret.txt}}",
            &workspace.path,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("absolute paths are not allowed"),
            "expected absolute-path rejection, got: {err}"
        );
    }

    #[test]
    fn rejects_unc_path_at_compile_time() {
        let workspace = TestWorkspace::new();
        let err = resolve_imports_inline(
            r"{{#runtime-import \\server\share\file.md}}",
            &workspace.path,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("absolute paths are not allowed"),
            "expected absolute-path rejection, got: {err}"
        );
    }

    #[test]
    fn resolves_multiple_markers_in_one_body() {
        let workspace = TestWorkspace::new();
        workspace.write("one.md", "ONE");
        workspace.write("two.md", "TWO");

        let result = resolve_imports_inline(
            "A {{#runtime-import one.md}} B {{#runtime-import two.md}} C",
            &workspace.path,
        )
        .unwrap();

        assert_eq!(result, "A ONE B TWO C");
    }

    /// `}` rejection keeps the compile-time resolver in strict parity
    /// with the runtime regex (`[^\s}]+`). Without this guard, a path
    /// like `foo}bar.md` would be accepted at compile time but cause
    /// the runtime resolver to either truncate it or leave the marker
    /// unexpanded — silent divergence. Reject up front.
    #[test]
    fn rejects_path_containing_closing_brace() {
        let workspace = TestWorkspace::new();
        let err = resolve_imports_inline(
            "{{#runtime-import foo}bar.md}}",
            &workspace.path,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("is not allowed"),
            "expected `}}` rejection, got: {err}"
        );
    }

    /// Path traversal: `..` segments would let a malicious agent body
    /// reach files outside `base_dir` (e.g. `../../../../etc/passwd` when
    /// `ado-aw compile` runs over an untrusted PR branch). Reject at
    /// resolution time regardless of whether the file actually exists.
    #[test]
    fn rejects_relative_path_with_dotdot_segment() {
        let workspace = TestWorkspace::new();
        let err = resolve_imports_inline(
            "{{#runtime-import ../escape.md}}",
            &workspace.path,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("'..' path components are not allowed"),
            "expected '..' rejection, got: {err}"
        );
    }

    #[test]
    fn rejects_path_with_embedded_dotdot_segment() {
        let workspace = TestWorkspace::new();
        let err = resolve_imports_inline(
            "{{#runtime-import sub/../../escape.md}}",
            &workspace.path,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("'..' path components are not allowed"),
            "expected '..' rejection, got: {err}"
        );
    }

    #[test]
    fn rejects_absolute_path_with_dotdot_segment() {
        let workspace = TestWorkspace::new();
        // The `..`-segment guard fires before the absolute-path guard,
        // so an absolute path with embedded `..` is reported as a
        // traversal violation. Either rejection is acceptable for this
        // input shape.
        let err = resolve_imports_inline(
            "{{#runtime-import /tmp/agents/../../etc/passwd}}",
            &workspace.path,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("'..' path components are not allowed"),
            "expected '..' rejection, got: {err}"
        );
    }

    #[test]
    fn rejects_backslash_dotdot_segment_on_windows_style_paths() {
        let workspace = TestWorkspace::new();
        let err = resolve_imports_inline(
            r"{{#runtime-import sub\..\..\escape.md}}",
            &workspace.path,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("'..' path components are not allowed"),
            "expected '..' rejection, got: {err}"
        );
    }

    /// `..filename.md` and `name..md` are not path-traversal — they're
    /// literal filenames where `..` is part of the name, not a segment.
    /// Make sure the segment-aware check doesn't false-positive on these.
    #[test]
    fn allows_literal_double_dot_in_filename() {
        let workspace = TestWorkspace::new();
        workspace.write("..hidden.md", "DOTHIDDEN");
        workspace.write("name..md", "DOUBLE");

        let a = resolve_imports_inline(
            "{{#runtime-import ..hidden.md}}",
            &workspace.path,
        )
        .unwrap();
        let b = resolve_imports_inline(
            "{{#runtime-import name..md}}",
            &workspace.path,
        )
        .unwrap();

        assert_eq!(a, "DOTHIDDEN");
        assert_eq!(b, "DOUBLE");
    }
}
