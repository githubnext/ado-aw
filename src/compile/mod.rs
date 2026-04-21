//! Pipeline compilation module.
//!
//! This module provides compilation of agent markdown files into Azure DevOps pipeline YAML.
//! Two targets are supported, both sharing the same execution model (Copilot CLI + AWF + MCPG):
//!
//! - **Standalone**: Self-contained pipeline with AWF network isolation
//! - **1ES**: Integration with 1ES Pipeline Templates for SDL compliance

mod common;
pub mod extensions;
mod onees;
mod standalone;
pub mod types;

use anyhow::{Context, Result};
use async_trait::async_trait;
use log::{debug, info};
use std::path::{Path, PathBuf};

pub use common::parse_markdown;
pub use common::HEADER_MARKER;
pub use common::generate_copilot_params;
pub use common::generate_mcpg_config;
pub use common::MCPG_IMAGE;
pub use common::MCPG_VERSION;
pub use common::MCPG_PORT;
pub use types::{CompileTarget, FrontMatter};

/// Trait for pipeline compilers.
///
/// Each target implements this trait to generate target-specific pipeline YAML.
#[async_trait]
pub trait Compiler: Send + Sync {
    /// Compile the front matter and markdown body into pipeline YAML.
    async fn compile(
        &self,
        input_path: &Path,
        output_path: &Path,
        front_matter: &FrontMatter,
        markdown_body: &str,
        skip_integrity: bool,
        debug_pipeline: bool,
    ) -> Result<String>;

    /// Get the target name for logging.
    fn target_name(&self) -> &'static str;
}

/// Main compilation function - entry point for the CLI.
///
/// Parses the input markdown file, determines the target, and delegates to the appropriate compiler.
pub async fn compile_pipeline(
    input_path: &str,
    output_path: Option<&str>,
    skip_integrity: bool,
    debug_pipeline: bool,
) -> Result<()> {
    let input_path = Path::new(input_path);
    info!("Compiling pipeline from: {}", input_path.display());

    // Read and parse input markdown
    debug!("Reading input file");
    let content = tokio::fs::read_to_string(input_path)
        .await
        .with_context(|| format!("Failed to read input file: {}", input_path.display()))?;
    debug!("Input file size: {} bytes", content.len());

    let (mut front_matter, markdown_body) = parse_markdown(&content)?;

    // Sanitize all front matter text fields before any further processing.
    // This neutralizes pipeline command injection (##vso[), strips control
    // characters, and enforces content limits across all config values.
    use crate::sanitize::SanitizeConfig;
    front_matter.sanitize_config_fields();

    info!("Parsed agent: '{}'", front_matter.name);
    debug!("Description: {}", front_matter.description);
    debug!("Target: {:?}", front_matter.target);
    debug!("Engine model: {}", front_matter.engine.model());
    debug!("Schedule: {:?}", front_matter.schedule);
    debug!("Repositories: {}", front_matter.repositories.len());
    debug!("MCP servers configured: {}", front_matter.mcp_servers.len());

    // Validate checkout list against repositories
    common::validate_checkout_list(&front_matter.repositories, &front_matter.checkout)?;

    // Determine output path
    let yaml_output_path = match output_path {
        Some(p) => PathBuf::from(p),
        None => input_path.with_extension("yml"),
    };

    // Select compiler based on target
    let compiler: Box<dyn Compiler> = match front_matter.target {
        CompileTarget::OneES => Box::new(onees::OneESCompiler),
        CompileTarget::Standalone => Box::new(standalone::StandaloneCompiler),
    };

    info!("Using {} compiler", compiler.target_name());

    // Compile
    let pipeline_yaml = compiler
        .compile(input_path, &yaml_output_path, &front_matter, &markdown_body, skip_integrity, debug_pipeline)
        .await?;

    // Clean up spacing artifacts from empty placeholder replacements
    let pipeline_yaml = clean_generated_yaml(&pipeline_yaml);

    // Write output
    tokio::fs::write(&yaml_output_path, &pipeline_yaml)
        .await
        .with_context(|| {
            format!(
                "Failed to write pipeline YAML: {}",
                yaml_output_path.display()
            )
        })?;

    println!(
        "Generated {} pipeline: {}",
        compiler.target_name(),
        yaml_output_path.display()
    );

    Ok(())
}

/// Auto-discover and recompile all agentic pipelines in the current directory.
///
/// Scans for compiled YAML files containing the `# @ado-aw source=...` header,
/// resolves each source markdown path, and recompiles. Pipelines whose source
/// files are missing are reported but don't abort the batch.
pub async fn compile_all_pipelines(skip_integrity: bool, debug_pipeline: bool) -> Result<()> {
    let root = Path::new(".");
    info!("Auto-discovering agentic pipelines for recompilation");

    let detected = crate::detect::detect_pipelines(root).await?;

    if detected.is_empty() {
        println!("No agentic pipelines found in the current directory.");
        println!("To compile a single file, run: ado-aw compile <path>");
        return Ok(());
    }

    println!("Found {} agentic pipeline(s):", detected.len());
    for p in &detected {
        println!(
            "  {} (source: {}, version: {})",
            p.yaml_path.display(),
            p.source,
            p.version
        );
    }
    println!();

    let mut success_count = 0;
    let mut skip_count = 0;
    let mut fail_count = 0;

    for pipeline in &detected {
        let source_path = root.join(&pipeline.source);
        let yaml_output_path = root.join(&pipeline.yaml_path);

        if !source_path.exists() {
            eprintln!(
                "  Warning: source '{}' not found for {}, skipping",
                pipeline.source,
                pipeline.yaml_path.display()
            );
            skip_count += 1;
            continue;
        }

        let source_str = source_path.to_string_lossy();
        let output_str = yaml_output_path.to_string_lossy();

        match compile_pipeline(&source_str, Some(&output_str), skip_integrity, debug_pipeline).await {
            Ok(()) => success_count += 1,
            Err(e) => {
                eprintln!(
                    "  Error compiling '{}': {}",
                    pipeline.source, e
                );
                fail_count += 1;
            }
        }
    }

    println!();
    println!(
        "Done: {} compiled, {} skipped, {} failed.",
        success_count, skip_count, fail_count
    );

    if fail_count > 0 {
        anyhow::bail!("{} pipeline(s) failed to compile", fail_count);
    }

    Ok(())
}

/// Check that a compiled pipeline YAML matches its source markdown.
///
/// Reads the `@ado-aw` header from `pipeline_path` to discover the source
/// markdown file, compiles it fresh, and compares the canonicalized output
/// against the existing pipeline file. When they differ, a unified diff is
/// printed to stderr showing exactly which lines changed.
pub async fn check_pipeline(pipeline_path: &str) -> Result<()> {
    let pipeline_path = Path::new(pipeline_path);

    // Read existing pipeline and extract header to discover source path
    let existing = tokio::fs::read_to_string(pipeline_path)
        .await
        .with_context(|| {
            format!(
                "Failed to read pipeline file: {}",
                pipeline_path.display()
            )
        })?;

    let header_meta = existing
        .lines()
        .take(5)
        .find_map(|line| crate::detect::parse_header_line(line))
        .with_context(|| {
            format!(
                "No @ado-aw header found in {}. Is this file generated by ado-aw?",
                pipeline_path.display()
            )
        })?;

    // Warn if the pipeline was generated by a different compiler version
    let current_version = env!("CARGO_PKG_VERSION");
    if !header_meta.version.is_empty() && header_meta.version != current_version {
        eprintln!(
            "Warning: pipeline was generated by ado-aw v{}, current version is v{}. \
             Version differences may cause expected changes in the output.",
            header_meta.version, current_version
        );
    }

    // The header stores the source path relative to the repository root.
    // Walk up from the pipeline file to find the .git directory, then resolve
    // the source path relative to that root.
    let pipeline_abs = if pipeline_path.is_absolute() {
        pipeline_path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(pipeline_path)
    };
    let repo_root = find_repo_root(&pipeline_abs).with_context(|| {
        format!(
            "Could not find repository root (no .git directory) from {}",
            pipeline_path.display()
        )
    })?;
    let source_path = repo_root.join(&header_meta.source);

    info!(
        "Checking pipeline integrity: {} -> {} (source from header)",
        source_path.display(),
        pipeline_path.display()
    );

    // Compile fresh from source
    let content = tokio::fs::read_to_string(&source_path)
        .await
        .with_context(|| {
            format!(
                "Source file '{}' (from header) not found. Has it been moved or deleted?",
                source_path.display()
            )
        })?;

    let (mut front_matter, markdown_body) = parse_markdown(&content)?;

    use crate::sanitize::SanitizeConfig;
    front_matter.sanitize_config_fields();

    common::validate_checkout_list(&front_matter.repositories, &front_matter.checkout)?;

    let compiler: Box<dyn Compiler> = match front_matter.target {
        CompileTarget::OneES => Box::new(onees::OneESCompiler),
        CompileTarget::Standalone => Box::new(standalone::StandaloneCompiler),
    };

    // Pass the header's relative source path to compile so the generated
    // header embeds the same path that was used during the original compilation.
    let pipeline_yaml = compiler
        .compile(
            Path::new(&header_meta.source),
            pipeline_path,
            &front_matter,
            &markdown_body,
            false,
            false,
        )
        .await?;

    // Canonicalize both sides: trim trailing whitespace, collapse blank lines,
    // but preserve indentation and internal spaces for meaningful comparison.
    let expected = clean_generated_yaml(&pipeline_yaml);
    let existing = clean_generated_yaml(&existing);

    if expected != existing {
        let diff_output = format_diff(&existing, &expected, pipeline_path);
        eprintln!("{}", diff_output);

        anyhow::bail!(
            "Integrity check failed: generated pipeline for '{}' does not match {}. \
             Re-run `ado-aw compile` to update the pipeline file.",
            front_matter.name,
            pipeline_path.display()
        );
    }

    println!("OK: {} is up to date", pipeline_path.display());
    Ok(())
}

/// Maximum number of changed lines to display in the diff output.
/// Keeps terminal output manageable for large pipeline files.
const MAX_DIFF_CHANGED_LINES: usize = 80;

/// Format a unified-style diff between the existing and expected pipeline content.
///
/// Shows changed lines with surrounding context and a summary of the total
/// number of changes. Output is truncated after [`MAX_DIFF_CHANGED_LINES`]
/// changed lines to avoid overwhelming the terminal.
fn format_diff(existing: &str, expected: &str, pipeline_path: &Path) -> String {
    use similar::{ChangeTag, TextDiff};

    let diff = TextDiff::from_lines(existing, expected);
    let mut output = String::new();

    output.push_str(&format!(
        "\n--- {} (on disk)\n+++ {} (expected from source)\n",
        pipeline_path.display(),
        pipeline_path.display(),
    ));

    // First pass: count total changes across the full diff.
    let (total_added, total_removed) = diff.iter_all_changes().fold((0usize, 0usize), |(a, r), c| {
        match c.tag() {
            ChangeTag::Insert => (a + 1, r),
            ChangeTag::Delete => (a, r + 1),
            ChangeTag::Equal => (a, r),
        }
    });

    let mut changed_lines_shown = 0usize;
    let mut truncated = false;

    for hunk in diff.unified_diff().context_radius(3).iter_hunks() {
        if truncated {
            break;
        }

        // Buffer hunk lines so we only emit the header if we have content to show.
        // This avoids orphaned hunk headers when truncation fires mid-hunk.
        let mut hunk_buf = String::new();

        for change in hunk.iter_changes() {
            let tag = change.tag();
            let line = change.value();

            if tag != ChangeTag::Equal {
                if changed_lines_shown >= MAX_DIFF_CHANGED_LINES {
                    truncated = true;
                    break;
                }
                changed_lines_shown += 1;
            }

            let prefix = match tag {
                ChangeTag::Delete => "-",
                ChangeTag::Insert => "+",
                ChangeTag::Equal => " ",
            };
            // Lines from TextDiff include trailing newlines; write directly.
            if line.ends_with('\n') {
                hunk_buf.push_str(&format!("{}{}", prefix, line));
            } else {
                hunk_buf.push_str(&format!("{}{}\n", prefix, line));
            }
        }

        if !hunk_buf.is_empty() {
            output.push_str(&format!("{}\n", hunk.header()));
            output.push_str(&hunk_buf);
        }
    }

    if truncated {
        output.push_str(&format!(
            "\n... diff truncated after {} changed lines (showing {} of {} total changes)\n",
            MAX_DIFF_CHANGED_LINES,
            changed_lines_shown,
            total_added + total_removed,
        ));
    }

    output.push_str(&format!(
        "\nSummary: {} line(s) added, {} line(s) removed\n",
        total_added, total_removed
    ));

    output
}

/// Walk up from `start` to find the nearest directory containing `.git`.
fn find_repo_root(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    loop {
        if current.join(".git").exists() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

/// Clean up spacing artifacts in generated YAML.
///
/// After template placeholder replacement, empty placeholders leave behind
/// trailing whitespace and consecutive blank lines. This function:
/// 1. Strips trailing whitespace from each line
/// 2. Collapses runs of blank lines into a single blank line
/// 3. Trims leading/trailing blank lines from the file
fn clean_generated_yaml(yaml: &str) -> String {
    let mut result = Vec::new();
    let mut prev_blank = false;

    for line in yaml.lines() {
        let trimmed_end = line.trim_end();
        let is_blank = trimmed_end.is_empty();

        if is_blank && prev_blank {
            continue;
        }

        result.push(trimmed_end);
        prev_blank = is_blank;
    }

    // Trim leading/trailing blank lines
    while result.first().is_some_and(|l| l.is_empty()) {
        result.remove(0);
    }
    while result.last().is_some_and(|l| l.is_empty()) {
        result.pop();
    }

    let mut output = result.join("\n");
    output.push('\n');
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_front_matter() {
        let content = r#"---
name: "Test Agent"
description: "A test agent"
---

## Instructions

Do something.
"#;
        let (fm, body) = parse_markdown(content).unwrap();
        assert_eq!(fm.name, "Test Agent");
        assert_eq!(fm.description, "A test agent");
        assert!(body.contains("## Instructions"));
    }

    #[test]
    fn test_parse_with_target() {
        let content = r#"---
name: "1ES Agent"
description: "Uses 1ES"
target: 1es
---

Body
"#;
        let (fm, _) = parse_markdown(content).unwrap();
        assert_eq!(fm.target, CompileTarget::OneES);
    }

    #[test]
    fn test_pool_string_format() {
        let content = r#"---
name: "Agent"
description: "Test"
pool: my-custom-pool
---
Body
"#;
        let (fm, _) = parse_markdown(content).unwrap();
        let pool = fm.pool.unwrap();
        assert_eq!(pool.name(), "my-custom-pool");
        assert_eq!(pool.os(), "linux"); // default
    }

    #[test]
    fn test_pool_object_format() {
        let content = r#"---
name: "Agent"
description: "Test"
pool:
  name: my-custom-pool
  os: windows
---
Body
"#;
        let (fm, _) = parse_markdown(content).unwrap();
        let pool = fm.pool.unwrap();
        assert_eq!(pool.name(), "my-custom-pool");
        assert_eq!(pool.os(), "windows");
    }

    #[test]
    fn test_schedule_string_form() {
        let content = r#"---
name: "Agent"
description: "Test"
schedule: daily around 14:00
---
Body
"#;
        let (fm, _) = parse_markdown(content).unwrap();
        let schedule = fm.schedule.unwrap();
        assert_eq!(schedule.expression(), "daily around 14:00");
        assert!(schedule.branches().is_empty());
    }

    #[test]
    fn test_schedule_object_form() {
        let content = r#"---
name: "Agent"
description: "Test"
schedule:
  run: weekly on friday around 17:00
  branches:
    - main
    - release/*
---
Body
"#;
        let (fm, _) = parse_markdown(content).unwrap();
        let schedule = fm.schedule.unwrap();
        assert_eq!(schedule.expression(), "weekly on friday around 17:00");
        assert_eq!(schedule.branches(), &["main", "release/*"]);
    }

    #[test]
    fn test_schedule_object_form_no_branches() {
        let content = r#"---
name: "Agent"
description: "Test"
schedule:
  run: daily
---
Body
"#;
        let (fm, _) = parse_markdown(content).unwrap();
        let schedule = fm.schedule.unwrap();
        assert_eq!(schedule.expression(), "daily");
        assert!(schedule.branches().is_empty());
    }

    #[test]
    fn test_generate_checkout_self_no_branch() {
        let result = common::generate_checkout_self();
        assert_eq!(result, "- checkout: self");
    }

    #[test]
    fn test_clean_generated_yaml_strips_trailing_whitespace() {
        let a = clean_generated_yaml("key: value\nother: data\n");
        let b = clean_generated_yaml("key: value  \nother: data  \n");
        assert_eq!(a, b);
    }

    #[test]
    fn test_clean_generated_yaml_collapses_blank_lines() {
        // Consecutive blank lines collapse to a single blank line
        let a = clean_generated_yaml("key: value\n\nother: data\n");
        let b = clean_generated_yaml("key: value\n\n\nother: data\n\n");
        assert_eq!(a, b);
    }

    #[test]
    fn test_clean_generated_yaml_preserves_indentation() {
        let input = "steps:\n  - bash: echo hello\n    displayName: greet\n";
        let cleaned = clean_generated_yaml(input);
        assert!(cleaned.contains("  - bash: echo hello"));
        assert!(cleaned.contains("    displayName: greet"));
    }

    #[test]
    fn test_clean_generated_yaml_preserves_internal_spaces() {
        let input = "script: echo a b c\n";
        let cleaned = clean_generated_yaml(input);
        assert!(cleaned.contains("echo a b c"));
    }

    #[test]
    fn test_format_diff_shows_added_lines() {
        let existing = "line1\nline2\n";
        let expected = "line1\nline2\nline3\n";
        let diff = format_diff(existing, expected, Path::new("test.yml"));
        assert!(diff.contains("+line3"));
        assert!(diff.contains("1 line(s) added"));
    }

    #[test]
    fn test_format_diff_shows_removed_lines() {
        let existing = "line1\nline2\nline3\n";
        let expected = "line1\nline3\n";
        let diff = format_diff(existing, expected, Path::new("test.yml"));
        assert!(diff.contains("-line2"));
        assert!(diff.contains("1 line(s) removed"));
    }

    #[test]
    fn test_format_diff_shows_changed_lines() {
        let existing = "key: old_value\nother: data\n";
        let expected = "key: new_value\nother: data\n";
        let diff = format_diff(existing, expected, Path::new("test.yml"));
        assert!(diff.contains("-key: old_value"));
        assert!(diff.contains("+key: new_value"));
        assert!(diff.contains("1 line(s) added, 1 line(s) removed"));
    }

    #[test]
    fn test_format_diff_identical_produces_no_hunks() {
        let content = "line1\nline2\n";
        let diff = format_diff(content, content, Path::new("test.yml"));
        assert!(diff.contains("0 line(s) added, 0 line(s) removed"));
        assert!(!diff.contains("@@"));
    }

    #[test]
    fn test_format_diff_includes_file_path() {
        let diff = format_diff("a\n", "b\n", Path::new("my-pipeline.yml"));
        assert!(diff.contains("my-pipeline.yml (on disk)"));
        assert!(diff.contains("my-pipeline.yml (expected from source)"));
    }

    #[test]
    fn test_format_diff_truncates_at_limit() {
        // 100 unique old lines replaced by 100 unique new lines = 200 total changes.
        let existing: String = (0..100).map(|i| format!("old{}\n", i)).collect();
        let expected: String = (0..100).map(|i| format!("new{}\n", i)).collect();
        let diff = format_diff(&existing, &expected, Path::new("test.yml"));
        assert!(
            diff.contains("... diff truncated"),
            "diff should be truncated for >80 changed lines"
        );
        // Exactly 80 changed lines shown, 200 total changes (100 removed + 100 added).
        assert!(
            diff.contains("showing 80 of 200 total changes"),
            "truncation message should report 80 shown of 200 total, got:\n{}",
            diff
        );
        // Summary should reflect ALL changes, not just the shown ones.
        assert!(
            diff.contains("100 line(s) added, 100 line(s) removed"),
            "summary should report full totals, got:\n{}",
            diff
        );
    }
}
