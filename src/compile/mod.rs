//! Pipeline compilation module.
//!
//! This module provides compilation of agent markdown files into Azure DevOps pipeline YAML.
//! Two targets are supported:
//!
//! - **Standalone**: Full-featured pipeline with custom network proxy, MCP firewall, and safe outputs
//! - **1ES**: Integration with 1ES Pipeline Templates using the agencyJob type

mod common;
mod onees;
mod standalone;
mod types;

use anyhow::{Context, Result};
use async_trait::async_trait;
use log::{debug, info};
use std::path::{Path, PathBuf};

pub use common::parse_markdown;
pub use common::sanitize_filename;
pub use common::HEADER_MARKER;
pub use types::{CompileTarget, FrontMatter, PermissionsConfig};

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
    ) -> Result<String>;

    /// Get the target name for logging.
    fn target_name(&self) -> &'static str;
}

/// Main compilation function - entry point for the CLI.
///
/// Parses the input markdown file, determines the target, and delegates to the appropriate compiler.
pub async fn compile_pipeline(input_path: &str, output_path: Option<&str>) -> Result<()> {
    let input_path = Path::new(input_path);
    info!("Compiling pipeline from: {}", input_path.display());

    // Read and parse input markdown
    debug!("Reading input file");
    let content = tokio::fs::read_to_string(input_path)
        .await
        .with_context(|| format!("Failed to read input file: {}", input_path.display()))?;
    debug!("Input file size: {} bytes", content.len());

    let (front_matter, markdown_body) = parse_markdown(&content)?;
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
        .compile(input_path, &yaml_output_path, &front_matter, &markdown_body)
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

/// Check that a compiled pipeline YAML matches its source markdown.
///
/// Compiles the source markdown fresh and compares (whitespace-normalized)
/// against the existing pipeline file. Returns an error if they differ.
pub async fn check_pipeline(source_path: &str, pipeline_path: &str) -> Result<()> {
    let source_path = Path::new(source_path);
    let pipeline_path = Path::new(pipeline_path);
    info!(
        "Checking pipeline integrity: {} -> {}",
        source_path.display(),
        pipeline_path.display()
    );

    // Compile fresh from source
    let content = tokio::fs::read_to_string(source_path)
        .await
        .with_context(|| format!("Failed to read source file: {}", source_path.display()))?;

    let (front_matter, markdown_body) = parse_markdown(&content)?;

    common::validate_checkout_list(&front_matter.repositories, &front_matter.checkout)?;

    let compiler: Box<dyn Compiler> = match front_matter.target {
        CompileTarget::OneES => Box::new(onees::OneESCompiler),
        CompileTarget::Standalone => Box::new(standalone::StandaloneCompiler),
    };

    let pipeline_yaml = compiler
        .compile(source_path, pipeline_path, &front_matter, &markdown_body)
        .await?;
    let pipeline_yaml = clean_generated_yaml(&pipeline_yaml);

    // Read existing pipeline file
    let existing = tokio::fs::read_to_string(pipeline_path)
        .await
        .with_context(|| {
            format!(
                "Failed to read pipeline file: {}",
                pipeline_path.display()
            )
        })?;

    // Compare ignoring whitespace differences
    if normalize_whitespace(&pipeline_yaml) != normalize_whitespace(&existing) {
        anyhow::bail!(
            "Integrity check failed: generated pipeline for '{}' does not match {}. \
             Re-run compilation to update the pipeline file.",
            front_matter.name,
            pipeline_path.display()
        );
    }

    println!("OK: {} is up to date", pipeline_path.display());
    Ok(())
}

/// Normalize a string by removing all whitespace characters.
///
/// Used for integrity checks so that formatting-only differences
/// (trailing spaces, blank lines, indentation changes) are ignored.
fn normalize_whitespace(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
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
    fn test_normalize_whitespace_strips_all_whitespace() {
        assert_eq!(normalize_whitespace("a b c"), "abc");
        assert_eq!(normalize_whitespace("a\n  b\n  c\n"), "abc");
        assert_eq!(normalize_whitespace("  hello  world  "), "helloworld");
    }

    #[test]
    fn test_normalize_whitespace_identical_content_matches() {
        let a = "key: value\nother: data\n";
        let b = "key: value\nother: data\n";
        assert_eq!(normalize_whitespace(a), normalize_whitespace(b));
    }

    #[test]
    fn test_normalize_whitespace_ignores_trailing_spaces() {
        let a = "key: value\nother: data\n";
        let b = "key: value  \nother: data  \n";
        assert_eq!(normalize_whitespace(a), normalize_whitespace(b));
    }

    #[test]
    fn test_normalize_whitespace_ignores_blank_lines() {
        let a = "key: value\nother: data\n";
        let b = "key: value\n\n\nother: data\n\n";
        assert_eq!(normalize_whitespace(a), normalize_whitespace(b));
    }

    #[test]
    fn test_normalize_whitespace_detects_content_difference() {
        let a = "key: value1\n";
        let b = "key: value2\n";
        assert_ne!(normalize_whitespace(a), normalize_whitespace(b));
    }
}
