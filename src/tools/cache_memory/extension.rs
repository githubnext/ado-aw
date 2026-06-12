use crate::compile::extensions::{CompileContext, CompilerExtension, Declarations, ExtensionPhase};
use crate::compile::ir::condition::Condition;
use crate::compile::ir::step::{BashStep, Step, TaskStep};
use crate::compile::types::CacheMemoryToolConfig;
use anyhow::Result;

/// Cache memory tool extension.
///
/// Injects: prepare steps (download/restore previous memory), and a
/// prompt supplement informing the agent about its memory directory.
pub struct CacheMemoryExtension {
    /// Config options (e.g., `allowed-extensions`) are consumed at Stage 3
    /// execution time, not at compile time. Retained here for potential
    /// future compile-time validation.
    #[allow(dead_code)]
    config: CacheMemoryToolConfig,
}

impl CacheMemoryExtension {
    pub fn new(config: CacheMemoryToolConfig) -> Self {
        Self { config }
    }
}

impl CompilerExtension for CacheMemoryExtension {
    fn name(&self) -> &str {
        "Cache Memory"
    }

    fn phase(&self) -> ExtensionPhase {
        ExtensionPhase::Tool
    }

    /// Typed-IR view. Returns three typed prepare steps in order:
    ///
    /// 1. [`Step::Task`] `DownloadPipelineArtifact@2` — fetches the
    ///    previous-run safe_outputs artifact (skipped via condition
    ///    when `clearMemory=true`).
    /// 2. [`Step::Bash`] — restores agent_memory from the downloaded
    ///    artifact (same condition).
    /// 3. [`Step::Bash`] — initialises an empty memory directory when
    ///    `clearMemory=true`.
    ///
    /// All three conditions reference the `clearMemory` template
    /// parameter via [`Condition::Custom`] (template expressions are
    /// not modelled natively in the IR's [`Condition`] AST; see the
    /// commit that introduced the AST for the rationale).
    fn declarations(&self, _ctx: &CompileContext) -> Result<Declarations> {
        Ok(Declarations {
            agent_prepare_steps: vec![
                Step::Task(download_previous_memory_task_step()),
                Step::Bash(restore_previous_memory_bash_step()),
                Step::Bash(initialize_empty_memory_bash_step()),
            ],
            prompt_supplement: Some(
                "\n\
---\n\
\n\
## Agent Memory\n\
\n\
You have persistent memory across runs. Your memory directory is located at `/tmp/awf-tools/staging/agent_memory/`.\n\
\n\
- **Read** previous memory files from this directory to recall context from prior runs.\n\
- **Write** new files or update existing ones in this directory to persist knowledge for future runs.\n\
- Use this memory to track patterns, accumulate findings, remember decisions, and improve over time.\n\
- The memory directory is yours to organize as you see fit (files, subdirectories, any structure).\n\
- Memory files are sanitized between runs for security; avoid including pipeline commands or secrets.\n"
                    .to_string(),
            ),
            ..Declarations::default()
        })
    }
}

/// Typed `DownloadPipelineArtifact@2` step that pulls the previous
/// safe_outputs artifact for the same pipeline+branch when
/// `clearMemory=false`.
fn download_previous_memory_task_step() -> TaskStep {
    let mut t = TaskStep::new(
        "DownloadPipelineArtifact@2",
        "Download previous agent memory",
    )
    .with_input("source", "specific")
    .with_input("project", "$(System.TeamProject)")
    .with_input("pipeline", "$(System.DefinitionId)")
    .with_input("runVersion", "latestFromBranch")
    .with_input("branchName", "$(Build.SourceBranch)")
    .with_input("artifact", "safe_outputs")
    .with_input("targetPath", "$(Agent.TempDirectory)/previous_memory")
    .with_input("allowPartiallySucceededBuilds", "true");
    t.condition = Some(Condition::Custom(
        "eq(${{ parameters.clearMemory }}, false)".to_string(),
    ));
    t.continue_on_error = true;
    t
}

/// Typed bash step that copies the downloaded agent_memory from the
/// previous_memory artifact into the staging directory. Runs only
/// when `clearMemory=false`.
fn restore_previous_memory_bash_step() -> BashStep {
    let script = "mkdir -p /tmp/awf-tools/staging/agent_memory\n\
                  if [ -d \"$(Agent.TempDirectory)/previous_memory/agent_memory\" ]; then\n  \
                    cp -a \"$(Agent.TempDirectory)/previous_memory/agent_memory/.\" /tmp/awf-tools/staging/agent_memory/ 2>/dev/null || true\n  \
                    echo \"Previous agent memory restored to /tmp/awf-tools/staging/agent_memory\"\n  \
                    ls -laR /tmp/awf-tools/staging/agent_memory\n\
                  else\n  \
                    echo \"No previous agent memory found - empty memory directory created\"\n\
                  fi\n";
    let mut b = BashStep::new("Restore previous agent memory", script).with_condition(
        Condition::Custom("eq(${{ parameters.clearMemory }}, false)".to_string()),
    );
    b.continue_on_error = true;
    b
}

/// Typed bash step that initialises an empty agent_memory directory
/// when the operator forces a fresh run via `clearMemory=true`.
fn initialize_empty_memory_bash_step() -> BashStep {
    let script = "mkdir -p /tmp/awf-tools/staging/agent_memory\n\
                  echo \"Memory cleared by pipeline parameter - starting fresh\"\n";
    BashStep::new("Initialize empty agent memory (clearMemory=true)", script).with_condition(
        Condition::Custom("eq(${{ parameters.clearMemory }}, true)".to_string()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::parse_markdown;

    fn make_ext() -> CacheMemoryExtension {
        CacheMemoryExtension::new(CacheMemoryToolConfig::Enabled(true))
    }

    /// Locks the `declarations()` override: must return exactly three
    /// typed steps (Task + two Bash) in the documented order, with
    /// the right conditions on each. Every step is typed.
    #[test]
    fn declarations_returns_three_typed_steps_with_clear_memory_conditions() {
        let (fm, _) = parse_markdown("---\nname: t\ndescription: x\n---\n").unwrap();
        let ext = make_ext();
        let ctx = CompileContext::for_test(&fm);
        let decl = ext.declarations(&ctx).unwrap();
        assert_eq!(decl.agent_prepare_steps.len(), 3);

        match &decl.agent_prepare_steps[0] {
            Step::Task(t) => {
                assert_eq!(t.task, "DownloadPipelineArtifact@2");
                assert_eq!(t.display_name, "Download previous agent memory");
                assert_eq!(
                    t.inputs.get("artifact").map(String::as_str),
                    Some("safe_outputs")
                );
                assert!(t.continue_on_error);
                match t.condition.as_ref().expect("condition required") {
                    Condition::Custom(s) => {
                        assert_eq!(s, "eq(${{ parameters.clearMemory }}, false)");
                    }
                    other => panic!("expected Condition::Custom, got {other:?}"),
                }
            }
            other => panic!("expected Step::Task(DownloadPipelineArtifact@2), got {other:?}"),
        }

        match &decl.agent_prepare_steps[1] {
            Step::Bash(b) => {
                assert_eq!(b.display_name, "Restore previous agent memory");
                assert!(b.script.contains("/tmp/awf-tools/staging/agent_memory"));
                assert!(b.continue_on_error);
                match b.condition.as_ref().expect("condition required") {
                    Condition::Custom(s) => {
                        assert_eq!(s, "eq(${{ parameters.clearMemory }}, false)");
                    }
                    other => panic!("expected Condition::Custom, got {other:?}"),
                }
            }
            other => panic!("expected Step::Bash(restore...), got {other:?}"),
        }

        match &decl.agent_prepare_steps[2] {
            Step::Bash(b) => {
                assert_eq!(
                    b.display_name,
                    "Initialize empty agent memory (clearMemory=true)"
                );
                assert!(b.script.contains("Memory cleared by pipeline parameter"));
                match b.condition.as_ref().expect("condition required") {
                    Condition::Custom(s) => {
                        assert_eq!(s, "eq(${{ parameters.clearMemory }}, true)");
                    }
                    other => panic!("expected Condition::Custom, got {other:?}"),
                }
            }
            other => panic!("expected Step::Bash(init...), got {other:?}"),
        }

        assert!(decl.prompt_supplement.is_some());
        assert!(decl.mcpg_servers.is_empty());
        assert!(decl.copilot_allow_tools.is_empty());
    }
}
