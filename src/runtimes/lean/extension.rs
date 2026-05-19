// ─── Lean 4 ──────────────────────────────────────────────────────────

use crate::compile::extensions::{AwfMount, AwfMountMode, CompileContext, CompilerExtension, ExtensionPhase};
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
}
