// ─── TLA+ ────────────────────────────────────────────────────────────

use super::{TLA_BASH_COMMANDS, TlaRuntimeConfig, DEFAULT_JDK_VERSION, generate_tla_tools_install};
use crate::compile::extensions::{
    AwfMount, AwfMountMode, CompileContext, CompilerExtension, Declarations, ExtensionPhase,
};
use crate::compile::ir::step::{BashStep, Step};
use crate::compile::ir::tasks::java_tool_installer::{JdkArchitecture, JavaToolInstaller};
use crate::validate;
use anyhow::Result;

/// TLA+ / TLC runtime extension.
///
/// Injects: bash commands (`java`, `tlc`, `pluscal`, `sany`), a
/// [`JavaToolInstaller@0`] task step (selects the pre-installed JDK and
/// sets `JAVA_HOME`), a bash step that downloads `tla2tools.jar` and
/// creates shims, AWF mounts (`$HOME/.tla` and `$(JAVA_HOME)`),
/// PATH prepends (`$HOME/.tla/bin` and `$(JAVA_HOME)/bin`), and a prompt
/// supplement.
///
/// GitHub is already in the built-in allowlist so no extra network host
/// entry is needed for the `tla2tools.jar` download.
///
/// [`JavaToolInstaller@0`]: crate::compile::ir::tasks::java_tool_installer::JavaToolInstaller
pub struct TlaExtension {
    config: TlaRuntimeConfig,
}

impl TlaExtension {
    pub fn new(config: TlaRuntimeConfig) -> Self {
        Self { config }
    }
}

impl CompilerExtension for TlaExtension {
    fn name(&self) -> &str {
        "TLA+"
    }

    fn phase(&self) -> ExtensionPhase {
        ExtensionPhase::Runtime
    }

    /// Returns two typed prepare steps and all static signals:
    ///
    /// 1. [`Step::Task`] — `JavaToolInstaller@0` (`PreInstalled` mode):
    ///    selects the JDK matching `runtimes.tla.jdk` and sets `JAVA_HOME`.
    /// 2. [`Step::Bash`] — downloads `tla2tools.jar` and creates shims
    ///    (`tlc`, `pluscal`, `sany`) that delegate to `java` from PATH.
    fn declarations(&self, ctx: &CompileContext) -> Result<Declarations> {
        let mut warnings = Vec::new();

        let is_bash_disabled = ctx
            .front_matter
            .tools
            .as_ref()
            .and_then(|t| t.bash.as_ref())
            .is_some_and(|cmds| cmds.is_empty());

        if is_bash_disabled {
            warnings.push(format!(
                "Agent '{}' has runtimes.tla enabled but tools.bash is empty. \
                 TLA+ requires bash access (java, tlc, pluscal, sany commands).",
                ctx.agent_name
            ));
        }

        // Validate version string (reject pipeline injection)
        if let Some(version) = self.config.version() {
            validate::reject_pipeline_injection(version, "runtimes.tla.version")?;
        }

        // Validate jdk string (reject pipeline injection)
        if let Some(jdk) = self.config.jdk() {
            validate::reject_pipeline_injection(jdk, "runtimes.tla.jdk")?;
        }

        Ok(Declarations {
            agent_prepare_steps: vec![
                Step::Task(java_install_task_step(&self.config)),
                Step::Bash(tla_tools_bash_step(&self.config)),
            ],
            // GitHub is already in the built-in allowlist — no extra entry
            // is needed for tla2tools.jar. The JDK is provided by
            // JavaToolInstaller@0 (pre-installed on the build agent).
            network_hosts: vec![],
            bash_commands: TLA_BASH_COMMANDS
                .iter()
                .map(|c| (*c).to_string())
                .collect(),
            prompt_supplement: Some(
                "\n\
---\n\
\n\
## TLA+ / TLC Formal Model Checking\n\
\n\
TLA+ is installed and available. The toolchain includes:\n\
- `tlc` — run the TLC model checker: `tlc -config M.cfg M.tla`\n\
- `pluscal` — translate PlusCal to TLA+: `pluscal -nocfg M.tla`\n\
- `sany` — parse and syntax-check a TLA+ spec: `sany M.tla`\n\
- `java` — direct JVM access: `java -cp \"$TLA_JAR\" tlc2.TLC ...`\n\
\n\
Environment variables available inside the agent sandbox:\n\
- `TLA_JAR` — absolute path to `tla2tools.jar`\n\
- `JAVA_HOME` — JDK home directory (set by `JavaToolInstaller@0`)\n\
\n\
TLA+ spec files use the `.tla` extension; PlusCal sources are embedded in\n\
`(*--algorithm ... *)` blocks inside `.tla` files.\n"
                    .to_string(),
            ),
            awf_mounts: vec![
                AwfMount::new("$HOME/.tla", "$HOME/.tla", AwfMountMode::ReadOnly),
                // Mount the JDK installation (path set at runtime by JavaToolInstaller@0)
                // into the AWF sandbox so `java` is accessible inside the chroot.
                AwfMount::new("$(JAVA_HOME)", "$(JAVA_HOME)", AwfMountMode::ReadOnly),
            ],
            awf_path_prepends: vec![
                "$HOME/.tla/bin".to_string(),
                // JavaToolInstaller@0 sets $(JAVA_HOME); expose its bin/ inside
                // the AWF chroot so shims can call `java` directly.
                "$(JAVA_HOME)/bin".to_string(),
            ],
            warnings,
            ..Declarations::default()
        })
    }
}

/// Build the typed [`crate::compile::ir::step::TaskStep`] for installing the JDK via `JavaToolInstaller@0`.
fn java_install_task_step(
    config: &TlaRuntimeConfig,
) -> crate::compile::ir::step::TaskStep {
    let jdk_version = config.jdk().unwrap_or(DEFAULT_JDK_VERSION);
    JavaToolInstaller::pre_installed(jdk_version, JdkArchitecture::X64).into_step()
}

/// Build the typed [`BashStep`] for downloading `tla2tools.jar` and creating shims.
fn tla_tools_bash_step(config: &TlaRuntimeConfig) -> BashStep {
    BashStep::new(
        "Install TLA+ toolchain (tla2tools.jar + shims)",
        generate_tla_tools_install(config),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::parse_markdown;

    #[test]
    fn test_validate_tla_bash_disabled_emits_warning() {
        let (fm, _) =
            parse_markdown("---\nname: test\ndescription: test\ntools:\n  bash: []\n---\n")
                .unwrap();
        let ext = TlaExtension::new(TlaRuntimeConfig::Enabled(true));
        let ctx = CompileContext::for_test(&fm);
        let warnings = ext.declarations(&ctx).unwrap().warnings;
        assert!(!warnings.is_empty());
        assert!(warnings[0].contains("tools.bash is empty"));
    }

    /// Locks `declarations()`: must return a typed `Step::Task` (`JavaToolInstaller@0`)
    /// followed by a `Step::Bash` (tla2tools.jar + shims), plus all static signals
    /// (bash commands, mounts, PATH prepends, prompt).
    #[test]
    fn declarations_returns_typed_task_then_bash_step_and_static_signals() {
        let (fm, _) = parse_markdown("---\nname: t\ndescription: x\n---\n").unwrap();
        let ext = TlaExtension::new(TlaRuntimeConfig::Enabled(true));
        let ctx = CompileContext::for_test(&fm);
        let decl = ext.declarations(&ctx).unwrap();
        assert_eq!(decl.agent_prepare_steps.len(), 2);
        // Step 1: JavaToolInstaller@0
        match &decl.agent_prepare_steps[0] {
            Step::Task(t) => {
                assert_eq!(t.task, "JavaToolInstaller@0");
                assert_eq!(
                    t.inputs.get("versionSpec").map(String::as_str),
                    Some("21"),
                    "default JDK version should be 21"
                );
                assert_eq!(
                    t.inputs.get("jdkSourceOption").map(String::as_str),
                    Some("PreInstalled")
                );
            }
            other => panic!("expected Step::Task(JavaToolInstaller@0), got {other:?}"),
        }
        // Step 2: tla2tools.jar + shims bash step
        match &decl.agent_prepare_steps[1] {
            Step::Bash(b) => {
                assert_eq!(
                    b.display_name,
                    "Install TLA+ toolchain (tla2tools.jar + shims)"
                );
                assert!(
                    !b.script.contains("api.adoptium.net"),
                    "should NOT fetch JRE from Adoptium (JavaToolInstaller handles it)"
                );
                assert!(b.script.contains("tla2tools.jar"), "should download tla2tools.jar");
                assert!(b.script.contains("tlc2.TLC"), "should create tlc shim");
                assert!(b.script.contains("pcal.trans"), "should create pluscal shim");
                assert!(b.script.contains("tla2sany.SANY"), "should create sany shim");
            }
            other => panic!("expected Step::Bash, got {other:?}"),
        }
        assert!(
            decl.network_hosts.is_empty(),
            "no extra network hosts needed: GitHub is built-in, JDK is pre-installed"
        );
        assert!(decl.bash_commands.contains(&"tlc".to_string()));
        assert!(decl.bash_commands.contains(&"java".to_string()));
        assert!(decl.bash_commands.contains(&"pluscal".to_string()));
        assert!(decl.bash_commands.contains(&"sany".to_string()));
        assert!(decl.prompt_supplement.is_some());
        assert_eq!(decl.awf_mounts.len(), 2);
        assert_eq!(decl.awf_mounts[0].host_path, "$HOME/.tla");
        assert_eq!(decl.awf_mounts[1].host_path, "$(JAVA_HOME)");
        assert_eq!(
            decl.awf_path_prepends,
            vec!["$HOME/.tla/bin".to_string(), "$(JAVA_HOME)/bin".to_string()]
        );
        // Slots TLA+ doesn't contribute to must be empty.
        assert!(decl.setup_steps.is_empty());
        assert!(decl.mcpg_servers.is_empty());
        assert!(decl.copilot_allow_tools.is_empty());
    }

    #[test]
    fn declarations_uses_default_jdk_and_latest_jar_when_unset() {
        let (fm, _) = parse_markdown("---\nname: t\ndescription: x\n---\n").unwrap();
        let ext = TlaExtension::new(TlaRuntimeConfig::Enabled(true));
        let ctx = CompileContext::for_test(&fm);
        let decl = ext.declarations(&ctx).unwrap();
        // Step 1: JavaToolInstaller with default JDK version
        match &decl.agent_prepare_steps[0] {
            Step::Task(t) => {
                assert_eq!(
                    t.inputs.get("versionSpec").map(String::as_str),
                    Some("21"),
                    "default JDK should be 21"
                );
            }
            other => panic!("expected Step::Task, got {other:?}"),
        }
        // Step 2: bash step with latest jar URL
        match &decl.agent_prepare_steps[1] {
            Step::Bash(b) => {
                assert!(
                    b.script.contains("releases/latest/download/tla2tools.jar"),
                    "default jar URL should use latest release redirect: {}",
                    b.script
                );
            }
            other => panic!("expected Step::Bash, got {other:?}"),
        }
    }

    #[test]
    fn declarations_uses_pinned_versions_when_configured() {
        let (fm, _) = parse_markdown(
            "---\nname: t\ndescription: x\nruntimes:\n  tla:\n    version: '1.8.0'\n    jdk: '17'\n---\n",
        )
        .unwrap();
        let tla = fm.runtimes.as_ref().unwrap().tla.as_ref().unwrap();
        let ext = TlaExtension::new(tla.clone());
        let ctx = CompileContext::for_test(&fm);
        let decl = ext.declarations(&ctx).unwrap();
        // Step 1: JavaToolInstaller with pinned JDK version
        match &decl.agent_prepare_steps[0] {
            Step::Task(t) => {
                assert_eq!(
                    t.inputs.get("versionSpec").map(String::as_str),
                    Some("17"),
                    "should use JDK 17"
                );
            }
            other => panic!("expected Step::Task, got {other:?}"),
        }
        // Step 2: bash step with pinned jar version
        match &decl.agent_prepare_steps[1] {
            Step::Bash(b) => {
                assert!(
                    b.script.contains("download/v1.8.0/tla2tools.jar"),
                    "should use pinned jar version 1.8.0: {}",
                    b.script
                );
            }
            other => panic!("expected Step::Bash, got {other:?}"),
        }
    }

    #[test]
    fn declarations_rejects_injection_in_version() {
        let (fm, _) = parse_markdown(
            "---\nname: t\ndescription: x\nruntimes:\n  tla:\n    version: '$(SECRET)'\n---\n",
        )
        .unwrap();
        let tla = fm.runtimes.as_ref().unwrap().tla.as_ref().unwrap();
        let ext = TlaExtension::new(tla.clone());
        let ctx = CompileContext::for_test(&fm);
        assert!(ext.declarations(&ctx).is_err());
    }

    #[test]
    fn declarations_rejects_injection_in_jdk() {
        let (fm, _) = parse_markdown(
            "---\nname: t\ndescription: x\nruntimes:\n  tla:\n    jdk: '$(SECRET)'\n---\n",
        )
        .unwrap();
        let tla = fm.runtimes.as_ref().unwrap().tla.as_ref().unwrap();
        let ext = TlaExtension::new(tla.clone());
        let ctx = CompileContext::for_test(&fm);
        assert!(ext.declarations(&ctx).is_err());
    }
}
