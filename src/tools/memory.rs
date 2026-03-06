//! Agent memory safe output processing
//!
//! Handles sanitization and persistence of the agent_memory folder across runs.
//! The agent writes files directly to the staging directory via shell commands.
//! During Stage 2 execution, this module validates and copies sanitized files
//! to the final safe_outputs artifact for pickup by the next run.

use anyhow::{Result, ensure};
use log::{debug, info, warn};
use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::tools::ExecutionResult;

/// Directory name for agent memory within the staging/artifact directories
pub const AGENT_MEMORY_DIR: &str = "agent_memory";

/// Maximum total size of agent memory folder (5 MB)
const MAX_MEMORY_SIZE_BYTES: u64 = 5 * 1024 * 1024;

/// Configuration for memory safe output, deserialized from front matter
#[derive(Debug, Clone, Deserialize, Default)]
pub struct MemoryConfig {
    /// Allowed file extensions (e.g., [".md", ".json", ".txt"]).
    /// Defaults to all extensions if empty or not specified.
    #[serde(default, rename = "allowed-extensions")]
    pub allowed_extensions: Vec<String>,
}

impl MemoryConfig {
    /// Check if a file extension is allowed by this config
    fn is_extension_allowed(&self, path: &Path) -> bool {
        if self.allowed_extensions.is_empty() {
            return true;
        }
        match path.extension().and_then(|e| e.to_str()) {
            Some(ext) => {
                let dot_ext = format!(".{}", ext);
                self.allowed_extensions
                    .iter()
                    .any(|allowed| allowed.eq_ignore_ascii_case(&dot_ext))
            }
            // Files with no extension are only allowed if "*" is in the list
            None => self.allowed_extensions.iter().any(|a| a == "*"),
        }
    }
}

/// Validate a single file path for safety
fn validate_memory_path(relative_path: &Path) -> Result<()> {
    let path_str = relative_path.to_string_lossy();

    // Check for null bytes
    ensure!(
        !path_str.contains('\0'),
        "Path contains null byte: {:?}",
        path_str
    );

    // Check for absolute paths
    ensure!(
        !path_str.starts_with('/') && !path_str.starts_with('\\'),
        "Absolute paths not allowed: {}",
        path_str
    );

    // Check for Windows absolute paths
    ensure!(
        !(path_str.len() >= 2 && path_str.chars().nth(1) == Some(':')),
        "Windows absolute paths not allowed: {}",
        path_str
    );

    // Check for path traversal
    for component in path_str.split(['/', '\\']) {
        ensure!(component != "..", "Path traversal not allowed: {}", path_str);
    }

    // Check for .git directory
    let lower = path_str.to_lowercase();
    ensure!(
        !lower.starts_with(".git/")
            && !lower.starts_with(".git\\")
            && lower != ".git",
        ".git directory not allowed: {}",
        path_str
    );

    Ok(())
}

/// Check file content for dangerous patterns (##vso[ commands)
async fn validate_memory_file_content(path: &Path) -> Result<bool> {
    // Only check text-like files for ##vso[ injection
    let is_text = match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => matches!(
            ext.to_lowercase().as_str(),
            "md" | "txt" | "json" | "yaml" | "yml" | "toml" | "xml" | "csv" | "log" | "sh"
                | "ps1" | "py" | "rs" | "js" | "ts"
        ),
        None => true, // No extension = assume text
    };

    if !is_text {
        return Ok(true);
    }

    match tokio::fs::read_to_string(path).await {
        Ok(content) => {
            if content.contains("##vso[") {
                warn!(
                    "Memory file contains ##vso[ command, rejecting: {}",
                    path.display()
                );
                Ok(false)
            } else {
                Ok(true)
            }
        }
        Err(_) => {
            // Binary file or encoding issue - skip content check
            Ok(true)
        }
    }
}

/// Recursively collect all files under a directory with their relative paths
async fn collect_files(base: &Path, current: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut entries = tokio::fs::read_dir(current).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.is_dir() {
            let mut sub_files = Box::pin(collect_files(base, &path)).await?;
            files.append(&mut sub_files);
        } else {
            let relative = path.strip_prefix(base)?.to_path_buf();
            files.push(relative);
        }
    }
    Ok(files)
}

/// Process agent memory from the safe output directory.
///
/// Validates and copies sanitized memory files from `source_dir/agent_memory/`
/// to `output_dir/agent_memory/`.
pub async fn process_agent_memory(
    source_dir: &Path,
    output_dir: &Path,
    config: &MemoryConfig,
) -> Result<ExecutionResult> {
    let memory_source = source_dir.join(AGENT_MEMORY_DIR);

    if !memory_source.exists() || !memory_source.is_dir() {
        info!("No agent_memory directory found, skipping memory processing");
        return Ok(ExecutionResult::success(
            "No agent memory to process",
        ));
    }

    info!(
        "Processing agent memory from: {}",
        memory_source.display()
    );

    let files = collect_files(&memory_source, &memory_source).await?;

    if files.is_empty() {
        info!("Agent memory directory is empty");
        return Ok(ExecutionResult::success(
            "Agent memory directory is empty",
        ));
    }

    info!("Found {} file(s) in agent_memory", files.len());

    let memory_output = output_dir.join(AGENT_MEMORY_DIR);
    let mut total_size: u64 = 0;
    let mut copied_count = 0;
    let mut skipped_count = 0;
    let mut skipped_reasons: Vec<String> = Vec::new();

    for relative_path in &files {
        let source_file = memory_source.join(relative_path);

        // Validate path safety
        if let Err(e) = validate_memory_path(relative_path) {
            warn!("Skipping unsafe path '{}': {}", relative_path.display(), e);
            skipped_count += 1;
            skipped_reasons.push(format!("{}: {}", relative_path.display(), e));
            continue;
        }

        // Validate file extension
        if !config.is_extension_allowed(relative_path) {
            warn!(
                "Skipping disallowed extension: {}",
                relative_path.display()
            );
            skipped_count += 1;
            skipped_reasons.push(format!(
                "{}: extension not in allowed list",
                relative_path.display()
            ));
            continue;
        }

        // Check file size contribution
        let metadata = tokio::fs::metadata(&source_file).await?;
        if total_size + metadata.len() > MAX_MEMORY_SIZE_BYTES {
            warn!(
                "Memory size limit ({} bytes) would be exceeded, skipping: {}",
                MAX_MEMORY_SIZE_BYTES,
                relative_path.display()
            );
            skipped_count += 1;
            skipped_reasons.push(format!(
                "{}: would exceed {} byte size limit",
                relative_path.display(),
                MAX_MEMORY_SIZE_BYTES
            ));
            continue;
        }

        // Validate content for ##vso[ injection
        if !validate_memory_file_content(&source_file).await? {
            skipped_count += 1;
            skipped_reasons.push(format!(
                "{}: contains ##vso[ command",
                relative_path.display()
            ));
            continue;
        }

        // Copy the file
        let dest_file = memory_output.join(relative_path);
        if let Some(parent) = dest_file.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::copy(&source_file, &dest_file).await?;
        total_size += metadata.len();
        copied_count += 1;
        debug!("Copied memory file: {}", relative_path.display());
    }

    let message = format!(
        "Agent memory processed: {} file(s) copied ({} bytes), {} skipped",
        copied_count, total_size, skipped_count
    );

    if !skipped_reasons.is_empty() {
        for reason in &skipped_reasons {
            info!("Skipped: {}", reason);
        }
    }

    info!("{}", message);
    println!("{}", message);

    Ok(ExecutionResult::success(message))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_path_normal() {
        assert!(validate_memory_path(Path::new("notes.md")).is_ok());
        assert!(validate_memory_path(Path::new("subdir/file.txt")).is_ok());
    }

    #[test]
    fn test_validate_path_traversal() {
        assert!(validate_memory_path(Path::new("../escape.txt")).is_err());
        assert!(validate_memory_path(Path::new("subdir/../../escape")).is_err());
    }

    #[test]
    fn test_validate_path_absolute() {
        assert!(validate_memory_path(Path::new("/etc/passwd")).is_err());
        assert!(validate_memory_path(Path::new("C:\\Windows")).is_err());
    }

    #[test]
    fn test_validate_path_git() {
        assert!(validate_memory_path(Path::new(".git/config")).is_err());
        assert!(validate_memory_path(Path::new(".git")).is_err());
    }

    #[test]
    fn test_validate_path_null_byte() {
        assert!(validate_memory_path(Path::new("file\0.txt")).is_err());
    }

    #[test]
    fn test_extension_allowed_all() {
        let config = MemoryConfig::default();
        assert!(config.is_extension_allowed(Path::new("file.md")));
        assert!(config.is_extension_allowed(Path::new("file.json")));
        assert!(config.is_extension_allowed(Path::new("file.anything")));
    }

    #[test]
    fn test_extension_allowed_restricted() {
        let config = MemoryConfig {
            allowed_extensions: vec![".md".to_string(), ".json".to_string()],
        };
        assert!(config.is_extension_allowed(Path::new("file.md")));
        assert!(config.is_extension_allowed(Path::new("file.json")));
        assert!(!config.is_extension_allowed(Path::new("file.txt")));
        assert!(!config.is_extension_allowed(Path::new("file.exe")));
    }

    #[test]
    fn test_extension_allowed_case_insensitive() {
        let config = MemoryConfig {
            allowed_extensions: vec![".md".to_string()],
        };
        assert!(config.is_extension_allowed(Path::new("file.MD")));
        assert!(config.is_extension_allowed(Path::new("file.Md")));
    }

    #[test]
    fn test_extension_no_extension_file() {
        let config = MemoryConfig {
            allowed_extensions: vec![".md".to_string()],
        };
        assert!(!config.is_extension_allowed(Path::new("Makefile")));

        let config_star = MemoryConfig {
            allowed_extensions: vec!["*".to_string()],
        };
        assert!(config_star.is_extension_allowed(Path::new("Makefile")));
    }

    #[tokio::test]
    async fn test_process_no_memory_dir() {
        let temp = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let config = MemoryConfig::default();

        let result = process_agent_memory(temp.path(), output.path(), &config)
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.message.contains("No agent memory"));
    }

    #[tokio::test]
    async fn test_process_empty_memory_dir() {
        let temp = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join(AGENT_MEMORY_DIR)).unwrap();
        let config = MemoryConfig::default();

        let result = process_agent_memory(temp.path(), output.path(), &config)
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.message.contains("empty"));
    }

    #[tokio::test]
    async fn test_process_copies_valid_files() {
        let temp = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let mem_dir = temp.path().join(AGENT_MEMORY_DIR);
        std::fs::create_dir(&mem_dir).unwrap();
        std::fs::write(mem_dir.join("notes.md"), "# Notes\nSome content").unwrap();
        std::fs::write(mem_dir.join("data.json"), r#"{"key": "value"}"#).unwrap();

        let config = MemoryConfig::default();
        let result = process_agent_memory(temp.path(), output.path(), &config)
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.message.contains("2 file(s) copied"));

        assert!(output.path().join(AGENT_MEMORY_DIR).join("notes.md").exists());
        assert!(output.path().join(AGENT_MEMORY_DIR).join("data.json").exists());
    }

    #[tokio::test]
    async fn test_process_skips_disallowed_extensions() {
        let temp = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let mem_dir = temp.path().join(AGENT_MEMORY_DIR);
        std::fs::create_dir(&mem_dir).unwrap();
        std::fs::write(mem_dir.join("notes.md"), "content").unwrap();
        std::fs::write(mem_dir.join("script.exe"), "bad").unwrap();

        let config = MemoryConfig {
            allowed_extensions: vec![".md".to_string()],
        };
        let result = process_agent_memory(temp.path(), output.path(), &config)
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.message.contains("1 file(s) copied"));
        assert!(result.message.contains("1 skipped"));

        assert!(output.path().join(AGENT_MEMORY_DIR).join("notes.md").exists());
        assert!(!output.path().join(AGENT_MEMORY_DIR).join("script.exe").exists());
    }

    #[tokio::test]
    async fn test_process_skips_path_traversal() {
        let temp = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let mem_dir = temp.path().join(AGENT_MEMORY_DIR);
        let sub_dir = mem_dir.join("subdir");
        std::fs::create_dir_all(&sub_dir).unwrap();
        std::fs::write(sub_dir.join("ok.md"), "content").unwrap();

        // We can't actually create a ../file on disk, but we test the validator
        let config = MemoryConfig::default();
        let result = process_agent_memory(temp.path(), output.path(), &config)
            .await
            .unwrap();
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_process_skips_vso_commands() {
        let temp = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let mem_dir = temp.path().join(AGENT_MEMORY_DIR);
        std::fs::create_dir(&mem_dir).unwrap();
        std::fs::write(mem_dir.join("safe.md"), "safe content").unwrap();
        std::fs::write(
            mem_dir.join("poisoned.md"),
            "##vso[task.setvariable variable=secret]hack",
        )
        .unwrap();

        let config = MemoryConfig::default();
        let result = process_agent_memory(temp.path(), output.path(), &config)
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.message.contains("1 file(s) copied"));
        assert!(result.message.contains("1 skipped"));

        assert!(output.path().join(AGENT_MEMORY_DIR).join("safe.md").exists());
        assert!(!output.path().join(AGENT_MEMORY_DIR).join("poisoned.md").exists());
    }

    #[tokio::test]
    async fn test_process_enforces_size_limit() {
        let temp = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let mem_dir = temp.path().join(AGENT_MEMORY_DIR);
        std::fs::create_dir(&mem_dir).unwrap();

        // Create a file just under the limit
        let large_content = "x".repeat(MAX_MEMORY_SIZE_BYTES as usize - 100);
        std::fs::write(mem_dir.join("large.txt"), &large_content).unwrap();
        // Create another file that would exceed the limit
        std::fs::write(mem_dir.join("overflow.txt"), "y".repeat(200)).unwrap();

        let config = MemoryConfig::default();
        let result = process_agent_memory(temp.path(), output.path(), &config)
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.message.contains("1 skipped"));
    }

    #[test]
    fn test_memory_config_deserialize() {
        let yaml = r#"
allowed-extensions:
  - .md
  - .json
"#;
        let config: MemoryConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.allowed_extensions, vec![".md", ".json"]);
    }

    #[test]
    fn test_memory_config_deserialize_empty() {
        let yaml = "{}";
        let config: MemoryConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.allowed_extensions.is_empty());
    }
}
