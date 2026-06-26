// ─── TLA+ ────────────────────────────────────────────────────────────

use super::{TLA_BASH_COMMANDS, TlaRuntimeConfig, generate_tla_install};
use crate::compile::extensions::{
    AwfMount, AwfMountMode, CompileContext, CompilerExtension, Declarations, ExtensionPhase,
};
use crate::compile::ir::step::{BashStep, Step};
use crate::validate;
use anyhow::Result;

/// TLA+ / TLC runtime extension.
///
/// Injects: network hosts (`java` ecosystem — covers Adoptium domains),
/// bash commands (`java`, `tlc`, `pluscal`, `sany`), install steps
/// (JRE download + `tla2tools.jar` + shims), AWF mounts (`$HOME/.tla`),
/// PATH prepend (`$HOME/.tla/bin`), and a prompt supplement.
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

    /// Returns the TLA+ install step as a [`Step::Bash`] alongside all
    /// static signals (hosts, bash commands, prompt supplement, AWF
    /// mounts, PATH prepends).
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
            agent_prepare_steps: vec![Step::Bash(tla_install_bash_step(&self.config))],
            // The `java` ecosystem identifier expands to all Adoptium/Temurin domains
            // (api.adoptium.net etc.). GitHub is already in the built-in allowlist so
            // no extra entry is needed for tla2tools.jar.
            network_hosts: vec!["java".to_string()],
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
- `TLA_JAVA_HOME` — JRE home directory\n\
\n\
TLA+ spec files use the `.tla` extension; PlusCal sources are embedded in\n\
`(*--algorithm ... *)` blocks inside `.tla` files.\n"
                    .to_string(),
            ),
            awf_mounts: vec![AwfMount::new(
                "$HOME/.tla",
                "$HOME/.tla",
                AwfMountMode::ReadOnly,
            )],
            awf_path_prepends: vec!["$HOME/.tla/bin".to_string()],
            warnings,
            ..Declarations::default()
        })
    }
}

/// Build the typed [`BashStep`] for installing the TLA+ toolchain.
fn tla_install_bash_step(config: &TlaRuntimeConfig) -> BashStep {
    BashStep::new("Install TLA+ toolchain (JRE + tla2tools.jar)", generate_tla_install(config))
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

    /// Locks `declarations()`: must return a single typed `Step::Bash` install
    /// step and all static signals (hosts, mounts, PATH prepends, prompt).
    #[test]
    fn declarations_returns_typed_bash_step_and_static_signals() {
        let (fm, _) = parse_markdown("---\nname: t\ndescription: x\n---\n").unwrap();
        let ext = TlaExtension::new(TlaRuntimeConfig::Enabled(true));
        let ctx = CompileContext::for_test(&fm);
        let decl = ext.declarations(&ctx).unwrap();
        assert_eq!(decl.agent_prepare_steps.len(), 1);
        match &decl.agent_prepare_steps[0] {
            Step::Bash(b) => {
                assert_eq!(b.display_name, "Install TLA+ toolchain (JRE + tla2tools.jar)");
                assert!(b.script.contains("api.adoptium.net"), "should fetch JRE from Adoptium");
                assert!(b.script.contains("tla2tools.jar"), "should download tla2tools.jar");
                assert!(b.script.contains("tlc2.TLC"), "should create tlc shim");
                assert!(b.script.contains("pcal.trans"), "should create pluscal shim");
                assert!(b.script.contains("tla2sany.SANY"), "should create sany shim");
            }
            other => panic!("expected Step::Bash, got {other:?}"),
        }
        assert_eq!(decl.network_hosts, vec!["java".to_string()]);
        assert!(decl.bash_commands.contains(&"tlc".to_string()));
        assert!(decl.bash_commands.contains(&"java".to_string()));
        assert!(decl.bash_commands.contains(&"pluscal".to_string()));
        assert!(decl.bash_commands.contains(&"sany".to_string()));
        assert!(decl.prompt_supplement.is_some());
        assert_eq!(decl.awf_mounts.len(), 1);
        assert_eq!(decl.awf_path_prepends, vec!["$HOME/.tla/bin".to_string()]);
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
        match &decl.agent_prepare_steps[0] {
            Step::Bash(b) => {
                assert!(
                    b.script.contains("/latest/21/"),
                    "default JDK should be 21: {}",
                    b.script
                );
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
        match &decl.agent_prepare_steps[0] {
            Step::Bash(b) => {
                assert!(
                    b.script.contains("/latest/17/"),
                    "should use JDK 17: {}",
                    b.script
                );
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
