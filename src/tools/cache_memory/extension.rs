use crate::compile::extensions::{CompilerExtension, ExtensionPhase};
use crate::compile::types::CacheMemoryToolConfig;

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

    fn prepare_steps(&self) -> Vec<String> {
        vec![generate_memory_download()]
    }

    fn prompt_supplement(&self) -> Option<String> {
        Some(
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
        )
    }
}

/// Generate the steps to download agent memory from the previous successful run
/// and restore it to the staging directory.
fn generate_memory_download() -> String {
    r#"- task: DownloadPipelineArtifact@2
  displayName: "Download previous agent memory"
  condition: eq(${{ parameters.clearMemory }}, false)
  continueOnError: true
  inputs:
    source: "specific"
    project: "$(System.TeamProject)"
    pipeline: "$(System.DefinitionId)"
    runVersion: "latestFromBranch"
    branchName: "$(Build.SourceBranch)"
    artifact: "safe_outputs"
    targetPath: "$(Agent.TempDirectory)/previous_memory"
    allowPartiallySucceededBuilds: true

- bash: |
    mkdir -p /tmp/awf-tools/staging/agent_memory
    if [ -d "$(Agent.TempDirectory)/previous_memory/agent_memory" ]; then
      cp -a "$(Agent.TempDirectory)/previous_memory/agent_memory/." /tmp/awf-tools/staging/agent_memory/ 2>/dev/null || true
      echo "Previous agent memory restored to /tmp/awf-tools/staging/agent_memory"
      ls -laR /tmp/awf-tools/staging/agent_memory
    else
      echo "No previous agent memory found - empty memory directory created"
    fi
  displayName: "Restore previous agent memory"
  condition: eq(${{ parameters.clearMemory }}, false)
  continueOnError: true

- bash: |
    mkdir -p /tmp/awf-tools/staging/agent_memory
    echo "Memory cleared by pipeline parameter - starting fresh"
  displayName: "Initialize empty agent memory (clearMemory=true)"
  condition: eq(${{ parameters.clearMemory }}, true)"#
        .to_string()
}
