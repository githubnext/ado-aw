// ─── Lean 4 ──────────────────────────────────────────────────────────

use crate::compile::extensions::{
    AwfMount, AwfMountMode, CompileContext, CompilerExtension, Declarations, ExtensionPhase,
};
use crate::compile::ir::step::{BashStep, Step};
use super::{LEAN_BASH_COMMANDS, LeanRuntimeConfig, generate_lean_install};
use anyhow::Result;

/// Lean 4 runtime extension.
///
/// Injects: network hosts (elan, lean-lang), bash commands (lean, lake,
/// elan), install steps (elan + toolchain), and a prompt supplement.
pub struct LeanExtension {
    config: LeanRuntimeConfig,
}

impl LeanExtension {
    pub fn new(config: LeanRuntimeConfig) -> Self {
        Self { config }
    }
}

impl CompilerExtension for LeanExtension {
    fn name(&self) -> &str {
        "Lean 4"
    }

    fn phase(&self) -> ExtensionPhase {
        ExtensionPhase::Runtime
    }

    fn required_hosts(&self) -> Vec<String> {
        vec!["lean".to_string()]
    }

    fn required_bash_commands(&self) -> Vec<String> {
        LEAN_BASH_COMMANDS
            .iter()
            .map(|c| (*c).to_string())
            .collect()
    }

    fn prompt_supplement(&self) -> Option<String> {
        Some(
            "\n\
---\n\
\n\
## Lean 4 Formal Verification\n\
\n\
Lean 4 is installed and available. Use `lean` to typecheck `.lean` files, \
`lake build` to build Lake projects, and `lake env printPaths` to inspect \
the toolchain. Lean files use the `.lean` extension.\n"
                .to_string(),
        )
    }

    fn prepare_steps(&self, _ctx: &CompileContext) -> Vec<String> {
        vec![generate_lean_install(&self.config)]
    }

    fn required_awf_mounts(&self) -> Vec<AwfMount> {
        vec![AwfMount::new("$HOME/.elan", "$HOME/.elan", AwfMountMode::ReadOnly)]
    }

    fn awf_path_prepends(&self) -> Vec<String> {
        vec!["$HOME/.elan/bin".to_string()]
    }

    fn validate(&self, ctx: &CompileContext) -> Result<Vec<String>> {
        let mut warnings = Vec::new();

        let is_bash_disabled = ctx
            .front_matter
            .tools
            .as_ref()
            .and_then(|t| t.bash.as_ref())
            .is_some_and(|cmds| cmds.is_empty());

        if is_bash_disabled {
            warnings.push(format!(
                "Agent '{}' has runtimes.lean enabled but tools.bash is empty. \
                 Lean requires bash access (lean, lake, elan commands).",
                ctx.agent_name
            ));
        }

        Ok(warnings)
    }

    /// Typed-IR view. Returns the single elan install step as a
    /// [`Step::Bash`] alongside all the static signals carried by the
    /// legacy accessors (hosts, bash commands, prompt supplement,
    /// AWF mounts, PATH prepends).
    ///
    /// Coexists with `prepare_steps` until the
    /// `compile-target-standalone` commit switches production
    /// consumption to `declarations`.
    fn declarations(&self, ctx: &CompileContext) -> Result<Declarations> {
        Ok(Declarations {
            agent_prepare_steps: vec![Step::Bash(lean_install_bash_step(&self.config))],
            network_hosts: self.required_hosts(),
            bash_commands: self.required_bash_commands(),
            prompt_supplement: self.prompt_supplement(),
            awf_mounts: self.required_awf_mounts(),
            awf_path_prepends: self.awf_path_prepends(),
            warnings: self.validate(ctx)?,
            ..Declarations::default()
        })
    }
}

/// Typed [`BashStep`] mirror of [`generate_lean_install`]. The script
/// body matches the legacy YAML body line-for-line so lowering through
/// `ir::emit` produces equivalent YAML.
fn lean_install_bash_step(config: &LeanRuntimeConfig) -> BashStep {
    let toolchain = config.toolchain().unwrap_or("stable");
    let script = format!(
        "set -eo pipefail\n\
         curl https://elan.lean-lang.org/elan-init.sh -sSf | sh -s -- -y --default-toolchain {toolchain}\n\
         echo \"##vso[task.prependpath]$HOME/.elan/bin\"\n\
         export PATH=\"$HOME/.elan/bin:$PATH\"\n\
         lean --version || echo \"Lean installed via elan\"\n\
         lake --version || echo \"Lake installed via elan\"\n"
    );
    BashStep::new("Install Lean 4 (elan)", script)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::parse_markdown;

    #[test]
    fn test_validate_lean_bash_disabled_emits_warning() {
        let (fm, _) =
            parse_markdown("---\nname: test\ndescription: test\ntools:\n  bash: []\n---\n")
                .unwrap();
        let ext = LeanExtension::new(LeanRuntimeConfig::Enabled(true));
        let ctx = CompileContext::for_test(&fm);
        let warnings = ext.validate(&ctx).unwrap();
        assert!(!warnings.is_empty());
        assert!(warnings[0].contains("tools.bash is empty"));
    }

    /// Locks the `declarations()` override against silent drift: must
    /// return a single typed `Step::Bash` install step (no
    /// `Step::RawYaml` migration bridge), and the static signals
    /// (hosts, mounts, PATH prepends, prompt) must all flow through.
    #[test]
    fn declarations_returns_typed_bash_step_and_static_signals() {
        let (fm, _) = parse_markdown("---\nname: t\ndescription: x\n---\n").unwrap();
        let ext = LeanExtension::new(LeanRuntimeConfig::Enabled(true));
        let ctx = CompileContext::for_test(&fm);
        let decl = ext.declarations(&ctx).unwrap();
        assert_eq!(decl.agent_prepare_steps.len(), 1);
        match &decl.agent_prepare_steps[0] {
            Step::Bash(b) => {
                assert_eq!(b.display_name, "Install Lean 4 (elan)");
                assert!(b.script.contains("elan-init.sh"));
                assert!(b.script.contains("--default-toolchain stable"));
            }
            other => panic!("expected Step::Bash, got {other:?}"),
        }
        assert_eq!(decl.network_hosts, vec!["lean".to_string()]);
        assert!(decl.bash_commands.contains(&"lean".to_string()));
        assert!(decl.prompt_supplement.is_some());
        assert_eq!(decl.awf_mounts.len(), 1);
        assert_eq!(decl.awf_path_prepends, vec!["$HOME/.elan/bin".to_string()]);
        // Slots Lean doesn't contribute to must be empty.
        assert!(decl.setup_steps.is_empty());
        assert!(decl.mcpg_servers.is_empty());
        assert!(decl.copilot_allow_tools.is_empty());
    }

    #[test]
    fn declarations_uses_pinned_toolchain_when_configured() {
        let (fm, _) = parse_markdown(
            "---\nname: t\ndescription: x\nruntimes:\n  lean:\n    toolchain: 'leanprover/lean4:v4.29.1'\n---\n",
        )
        .unwrap();
        let lean = fm.runtimes.as_ref().unwrap().lean.as_ref().unwrap();
        let ext = LeanExtension::new(lean.clone());
        let ctx = CompileContext::for_test(&fm);
        let decl = ext.declarations(&ctx).unwrap();
        match &decl.agent_prepare_steps[0] {
            Step::Bash(b) => assert!(
                b.script.contains("--default-toolchain leanprover/lean4:v4.29.1"),
                "expected pinned toolchain in script: {}",
                b.script
            ),
            other => panic!("expected Step::Bash, got {other:?}"),
        }
    }
}
