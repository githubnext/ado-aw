//! Agent memory safe output processing
//!
//! Handles sanitization and persistence of the agent_memory folder across runs.
//! The agent writes files directly to the staging directory via shell commands.
//! During Stage 3 execution, this module validates and copies sanitized files
//! to the final safe_outputs artifact for pickup by the next run.

use anyhow::{Context, Result, ensure};
use log::{debug, info, warn};
use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::safe_outputs::ExecutionResult;

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
        ensure!(
            component != "..",
            "Path traversal not allowed: {}",
            path_str
        );
    }

    // Check for .git directory
    let lower = path_str.to_lowercase();
    ensure!(
        !lower.starts_with(".git/") && !lower.starts_with(".git\\") && lower != ".git",
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
            "md" | "txt"
                | "json"
                | "yaml"
                | "yml"
                | "toml"
                | "xml"
                | "csv"
                | "log"
                | "sh"
                | "ps1"
                | "py"
                | "rs"
                | "js"
                | "ts"
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

/// Recursively collect all files under a directory with their relative paths.
///
/// Symlinks (both file and directory) are skipped with a warning to prevent
/// symlink-following attacks that could expose files outside the memory directory.
async fn collect_files(base: &Path, current: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut entries = tokio::fs::read_dir(current).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        // Use file_type() which does NOT follow symlinks (unlike path.is_dir()).
        let file_type = entry.file_type().await?;
        if file_type.is_symlink() {
            warn!("Skipping symlink in memory directory: {}", path.display());
        } else if file_type.is_dir() {
            // This branch is only reached when file_type.is_symlink() is false,
            // so `path` is guaranteed to be a real directory (not a symlink-to-dir).
            let mut sub_files = Box::pin(collect_files(base, &path)).await?;
            files.append(&mut sub_files);
        } else {
            let relative = path.strip_prefix(base)?.to_path_buf();
            files.push(relative);
        }
    }
    Ok(files)
}

/// Outcome of processing a single memory file.
enum FileOutcome {
    Copied(u64),
    Skipped(String),
}

/// Validate containment, safety, extension, size, and content for one file;
/// copy it to `dest_file` if all checks pass.
///
/// All validation failures return `Ok(FileOutcome::Skipped(reason))` so the
/// caller can keep a tally without complex nested control flow.
async fn process_memory_file(
    relative_path: &Path,
    source_file: &Path,
    canonical_base: &Path,
    dest_file: &Path,
    config: &MemoryConfig,
    total_size: u64,
) -> Result<FileOutcome> {
    // Defense-in-depth: verify the fully-resolved source path is still within
    // the memory base directory, guarding against TOCTOU symlink races.
    match tokio::fs::canonicalize(source_file).await {
        Ok(canonical_source) => {
            if !canonical_source.starts_with(canonical_base) {
                warn!(
                    "Skipping path that escapes memory directory (possible symlink attack): {}",
                    relative_path.display()
                );
                return Ok(FileOutcome::Skipped(format!(
                    "{}: resolved path escapes memory directory",
                    relative_path.display()
                )));
            }
        }
        Err(e) => {
            warn!(
                "Skipping unresolvable path '{}': {}",
                relative_path.display(),
                e
            );
            return Ok(FileOutcome::Skipped(format!(
                "{}: cannot resolve path ({})",
                relative_path.display(),
                e
            )));
        }
    }

    if let Err(e) = validate_memory_path(relative_path) {
        warn!("Skipping unsafe path '{}': {}", relative_path.display(), e);
        return Ok(FileOutcome::Skipped(format!(
            "{}: {}",
            relative_path.display(),
            e
        )));
    }

    if !config.is_extension_allowed(relative_path) {
        warn!("Skipping disallowed extension: {}", relative_path.display());
        return Ok(FileOutcome::Skipped(format!(
            "{}: extension not in allowed list",
            relative_path.display()
        )));
    }

    let metadata = tokio::fs::metadata(source_file).await?;
    if total_size + metadata.len() > MAX_MEMORY_SIZE_BYTES {
        warn!(
            "Memory size limit ({} bytes) would be exceeded, skipping: {}",
            MAX_MEMORY_SIZE_BYTES,
            relative_path.display()
        );
        return Ok(FileOutcome::Skipped(format!(
            "{}: would exceed {} byte size limit",
            relative_path.display(),
            MAX_MEMORY_SIZE_BYTES
        )));
    }

    if !validate_memory_file_content(source_file).await? {
        return Ok(FileOutcome::Skipped(format!(
            "{}: contains ##vso[ command",
            relative_path.display()
        )));
    }

    if let Some(parent) = dest_file.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::copy(source_file, dest_file).await?;
    debug!("Copied memory file: {}", relative_path.display());
    Ok(FileOutcome::Copied(metadata.len()))
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

    // Use symlink_metadata (lstat) so we do NOT follow a symlink at the base
    // directory level — a top-level `agent_memory -> /sensitive/dir` symlink
    // in the artifact would otherwise bypass all per-file containment checks.
    match tokio::fs::symlink_metadata(&memory_source).await {
        Err(_) => {
            info!("No agent_memory directory found, skipping memory processing");
            return Ok(ExecutionResult::success("No agent memory to process"));
        }
        Ok(m) if m.is_symlink() => {
            warn!(
                "agent_memory is a symlink — skipping to prevent directory escape: {}",
                memory_source.display()
            );
            return Ok(ExecutionResult::success("No agent memory to process"));
        }
        Ok(m) if !m.is_dir() => {
            info!("No agent_memory directory found, skipping memory processing");
            return Ok(ExecutionResult::success("No agent memory to process"));
        }
        Ok(_) => {} // real directory, proceed
    }

    info!("Processing agent memory from: {}", memory_source.display());

    let files = collect_files(&memory_source, &memory_source).await?;

    if files.is_empty() {
        info!("Agent memory directory is empty");
        return Ok(ExecutionResult::success("Agent memory directory is empty"));
    }

    info!("Found {} file(s) in agent_memory", files.len());

    // Canonicalize the base directory once for containment checks below.
    // This is required for the symlink-following defense: we compare canonical
    // resolved paths to ensure no file escapes the memory directory boundary.
    let canonical_base = tokio::fs::canonicalize(&memory_source).await.with_context(|| {
        format!(
            "Cannot verify memory directory containment (symlink defense requires canonical path): {}",
            memory_source.display()
        )
    })?;

    let memory_output = output_dir.join(AGENT_MEMORY_DIR);
    let mut total_size: u64 = 0;
    let mut copied_count = 0;
    let mut skipped_count = 0;
    let mut skipped_reasons: Vec<String> = Vec::new();

    for relative_path in &files {
        let source_file = memory_source.join(relative_path);
        let dest_file = memory_output.join(relative_path);
        match process_memory_file(
            relative_path,
            &source_file,
            &canonical_base,
            &dest_file,
            config,
            total_size,
        )
        .await?
        {
            FileOutcome::Copied(size) => {
                total_size += size;
                copied_count += 1;
            }
            FileOutcome::Skipped(reason) => {
                skipped_count += 1;
                skipped_reasons.push(reason);
            }
        }
    }

    let message = format!(
        "Agent memory processed: {} file(s) copied ({} bytes), {} skipped",
        copied_count, total_size, skipped_count
    );

    for reason in &skipped_reasons {
        info!("Skipped: {}", reason);
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

        assert!(
            output
                .path()
                .join(AGENT_MEMORY_DIR)
                .join("notes.md")
                .exists()
        );
        assert!(
            output
                .path()
                .join(AGENT_MEMORY_DIR)
                .join("data.json")
                .exists()
        );
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

        assert!(
            output
                .path()
                .join(AGENT_MEMORY_DIR)
                .join("notes.md")
                .exists()
        );
        assert!(
            !output
                .path()
                .join(AGENT_MEMORY_DIR)
                .join("script.exe")
                .exists()
        );
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

        assert!(
            output
                .path()
                .join(AGENT_MEMORY_DIR)
                .join("safe.md")
                .exists()
        );
        assert!(
            !output
                .path()
                .join(AGENT_MEMORY_DIR)
                .join("poisoned.md")
                .exists()
        );
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

    // --- Symlink security tests (Unix only) ---

    #[cfg(unix)]
    #[tokio::test]
    async fn test_collect_files_skips_file_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        let mem_dir = temp.path().join(AGENT_MEMORY_DIR);
        std::fs::create_dir(&mem_dir).unwrap();

        // A real file that should be collected
        std::fs::write(mem_dir.join("real.md"), "real content").unwrap();

        // A target file sitting outside the memory directory
        let target = temp.path().join("secret.txt");
        std::fs::write(&target, "secret data").unwrap();

        // Symlink inside mem_dir pointing to the outside target
        std::os::unix::fs::symlink(&target, mem_dir.join("symlink.txt")).unwrap();

        let files = collect_files(&mem_dir, &mem_dir).await.unwrap();

        // Only the real file should be collected; the symlink must be skipped
        assert_eq!(files.len(), 1, "symlink should be skipped");
        assert_eq!(files[0], std::path::PathBuf::from("real.md"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_collect_files_skips_relative_symlinks() {
        // Relative symlinks (e.g., ../../etc/passwd) are just as dangerous as
        // absolute ones and must also be rejected.
        let temp = tempfile::tempdir().unwrap();
        let mem_dir = temp.path().join(AGENT_MEMORY_DIR);
        std::fs::create_dir(&mem_dir).unwrap();

        // A real file
        std::fs::write(mem_dir.join("real.md"), "real content").unwrap();

        // Relative symlink: within mem_dir, "../secret.txt" points one level up
        let target = temp.path().join("secret.txt");
        std::fs::write(&target, "secret data").unwrap();
        std::os::unix::fs::symlink("../secret.txt", mem_dir.join("relative_link.txt")).unwrap();

        let files = collect_files(&mem_dir, &mem_dir).await.unwrap();

        // Only the real file should be collected; the relative symlink must be skipped
        assert_eq!(files.len(), 1, "relative symlink should be skipped");
        assert_eq!(files[0], std::path::PathBuf::from("real.md"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_collect_files_skips_directory_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        let mem_dir = temp.path().join(AGENT_MEMORY_DIR);
        std::fs::create_dir(&mem_dir).unwrap();

        // A target directory outside the memory dir containing a sensitive file
        let target_dir = temp.path().join("outside_dir");
        std::fs::create_dir(&target_dir).unwrap();
        std::fs::write(target_dir.join("secret.md"), "secret data").unwrap();

        // Directory symlink inside mem_dir pointing to the outside directory
        std::os::unix::fs::symlink(&target_dir, mem_dir.join("linked_dir")).unwrap();

        let files = collect_files(&mem_dir, &mem_dir).await.unwrap();

        // The directory symlink must not be recursed into; nothing should be collected
        assert_eq!(files.len(), 0, "directory symlink should not be followed");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_process_memory_skips_file_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let mem_dir = temp.path().join(AGENT_MEMORY_DIR);
        std::fs::create_dir(&mem_dir).unwrap();

        // A legitimate file
        std::fs::write(mem_dir.join("notes.md"), "safe notes").unwrap();

        // A sensitive file outside the memory dir (simulating /proc/self/environ)
        let sensitive = temp.path().join("environ");
        std::fs::write(&sensitive, "SECRET_TOKEN=hunter2\nOTHER=val").unwrap();

        // Symlink inside the memory dir pointing at the sensitive file
        std::os::unix::fs::symlink(&sensitive, mem_dir.join("env.txt")).unwrap();

        let config = MemoryConfig::default();
        let result = process_agent_memory(temp.path(), output.path(), &config)
            .await
            .unwrap();

        assert!(result.success);
        // Only notes.md should be copied; the symlink is silently dropped by collect_files
        assert!(
            result.message.contains("1 file(s) copied"),
            "unexpected message: {}",
            result.message
        );

        let out_memory = output.path().join(AGENT_MEMORY_DIR);
        assert!(
            out_memory.join("notes.md").exists(),
            "notes.md should be copied"
        );
        assert!(
            !out_memory.join("env.txt").exists(),
            "symlink target must not be copied"
        );

        // The sensitive contents must not appear in the output at all
        let out_notes = std::fs::read_to_string(out_memory.join("notes.md")).unwrap();
        assert!(!out_notes.contains("SECRET_TOKEN"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_process_memory_skips_directory_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let mem_dir = temp.path().join(AGENT_MEMORY_DIR);
        std::fs::create_dir(&mem_dir).unwrap();

        // A legitimate file
        std::fs::write(mem_dir.join("notes.md"), "safe notes").unwrap();

        // A directory outside the memory dir with sensitive contents
        let outside_dir = temp.path().join("outside");
        std::fs::create_dir(&outside_dir).unwrap();
        std::fs::write(outside_dir.join("passwd"), "root:x:0:0").unwrap();

        // Directory symlink inside the memory dir pointing outside
        std::os::unix::fs::symlink(&outside_dir, mem_dir.join("etc_backup")).unwrap();

        let config = MemoryConfig::default();
        let result = process_agent_memory(temp.path(), output.path(), &config)
            .await
            .unwrap();

        assert!(result.success);
        // Only notes.md should be copied
        assert!(
            result.message.contains("1 file(s) copied"),
            "unexpected message: {}",
            result.message
        );

        let out_memory = output.path().join(AGENT_MEMORY_DIR);
        assert!(out_memory.join("notes.md").exists());
        // The outside directory must not have been recursed into
        assert!(!out_memory.join("etc_backup").exists());
        assert!(!out_memory.join("etc_backup").join("passwd").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_process_memory_rejects_base_directory_symlink() {
        // If the entire agent_memory entry is a symlink to an outside directory,
        // all per-file containment checks would resolve relative to that target,
        // so every file inside would "pass" the starts_with guard. The fix is to
        // reject agent_memory itself when it is a symlink (lstat check).
        let temp = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();

        // Sensitive directory outside the staging area
        let outside_dir = temp.path().join("sensitive");
        std::fs::create_dir(&outside_dir).unwrap();
        std::fs::write(outside_dir.join("environ"), "SC_WRITE_TOKEN=secret").unwrap();

        // The whole agent_memory entry is a symlink to the sensitive directory
        std::os::unix::fs::symlink(&outside_dir, temp.path().join(AGENT_MEMORY_DIR)).unwrap();

        let config = MemoryConfig::default();
        let result = process_agent_memory(temp.path(), output.path(), &config)
            .await
            .unwrap();

        assert!(result.success);
        // Should be treated as "no memory" — nothing copied
        assert!(
            result.message.contains("No agent memory"),
            "unexpected message: {}",
            result.message
        );
        // Sensitive file must not appear in output
        assert!(
            !output
                .path()
                .join(AGENT_MEMORY_DIR)
                .join("environ")
                .exists()
        );
    }
}
