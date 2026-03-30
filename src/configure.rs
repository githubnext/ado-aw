//! The `configure` CLI command.
//!
//! Detects agentic pipelines in a local repository and updates the `GITHUB_TOKEN`
//! pipeline variable on their corresponding Azure DevOps build definitions.
//!
//! Uses the same ADO REST API patterns as the existing tools in `src/tools/`
//! (reqwest + `.basic_auth("", Some(token))` for authentication).

use anyhow::{Context, Result};
use log::{debug, info};
use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::detect;

// ==================== ADO context from git remote ====================

/// ADO context extracted from the git remote URL.
#[derive(Debug, Clone)]
pub struct AdoContext {
    /// Organization URL (e.g., `https://dev.azure.com/myorg`)
    pub org_url: String,
    /// Project name
    pub project: String,
    /// Repository name
    pub repo_name: String,
}

/// Parse the ADO org, project, and repo from a git remote URL.
///
/// Supports:
/// - HTTPS: `https://dev.azure.com/{org}/{project}/_git/{repo}`
/// - HTTPS (legacy): `https://{org}.visualstudio.com/{project}/_git/{repo}`
/// - SSH: `git@ssh.dev.azure.com:v3/{org}/{project}/{repo}`
pub fn parse_ado_remote(remote_url: &str) -> Result<AdoContext> {
    let url = remote_url.trim();

    // SSH format: git@ssh.dev.azure.com:v3/{org}/{project}/{repo}
    if let Some(rest) = url.strip_prefix("git@ssh.dev.azure.com:v3/") {
        let parts: Vec<&str> = rest.splitn(3, '/').collect();
        if parts.len() >= 3 {
            let repo_name = parts[2].trim_end_matches(".git");
            return Ok(AdoContext {
                org_url: format!("https://dev.azure.com/{}", parts[0]),
                project: parts[1].to_string(),
                repo_name: repo_name.to_string(),
            });
        }
    }

    // HTTPS format: https://dev.azure.com/{org}/{project}/_git/{repo}
    if url.contains("dev.azure.com") {
        let url_parsed =
            url::Url::parse(url).with_context(|| format!("Invalid URL: {}", url))?;
        let segments: Vec<&str> = url_parsed
            .path_segments()
            .map(|s| s.collect())
            .unwrap_or_default();

        // Expected: /{org}/{project}/_git/{repo}
        if segments.len() >= 4 && segments[2] == "_git" {
            let repo_name = segments[3].trim_end_matches(".git");
            return Ok(AdoContext {
                org_url: format!("https://dev.azure.com/{}", segments[0]),
                project: segments[1].to_string(),
                repo_name: repo_name.to_string(),
            });
        }
    }

    // Legacy format: https://{org}.visualstudio.com/{project}/_git/{repo}
    if url.contains(".visualstudio.com") {
        let url_parsed =
            url::Url::parse(url).with_context(|| format!("Invalid URL: {}", url))?;
        let host = url_parsed.host_str().unwrap_or("");
        let org = host.strip_suffix(".visualstudio.com").unwrap_or(host);
        let segments: Vec<&str> = url_parsed
            .path_segments()
            .map(|s| s.collect())
            .unwrap_or_default();

        // Expected: /{project}/_git/{repo}
        if segments.len() >= 3 && segments[1] == "_git" {
            let repo_name = segments[2].trim_end_matches(".git");
            return Ok(AdoContext {
                org_url: format!("https://dev.azure.com/{}", org),
                project: segments[0].to_string(),
                repo_name: repo_name.to_string(),
            });
        }
    }

    anyhow::bail!(
        "Could not parse ADO context from remote URL: {}. \
         Expected format: https://dev.azure.com/{{org}}/{{project}}/_git/{{repo}}",
        url
    )
}

/// Get the git remote URL for the repository at `repo_path`.
async fn get_git_remote_url(repo_path: &Path) -> Result<String> {
    let output = tokio::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(repo_path)
        .output()
        .await
        .context("Failed to run git command")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git remote get-url origin failed: {}", stderr);
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

// ==================== ADO Build Definitions API ====================

/// Minimal subset of an ADO Build Definition for listing.
#[derive(Debug, Deserialize)]
struct DefinitionListResponse {
    value: Vec<DefinitionSummary>,
}

#[derive(Debug, Deserialize)]
struct DefinitionSummary {
    id: u64,
    name: String,
    process: Option<ProcessInfo>,
}

#[derive(Debug, Deserialize)]
struct ProcessInfo {
    #[serde(rename = "yamlFilename")]
    yaml_filename: Option<String>,
}

/// How a local YAML file was matched to an ADO pipeline definition.
#[derive(Debug, Clone)]
pub enum MatchMethod {
    YamlPath,
    PipelineName,
}

impl std::fmt::Display for MatchMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MatchMethod::YamlPath => write!(f, "yaml-path"),
            MatchMethod::PipelineName => write!(f, "pipeline-name"),
        }
    }
}

/// A matched pipeline definition from ADO.
#[derive(Debug, Clone)]
struct MatchedDefinition {
    id: u64,
    name: String,
    match_method: MatchMethod,
    yaml_path: String,
}

/// List all build definitions in the project.
async fn list_definitions(
    client: &reqwest::Client,
    ctx: &AdoContext,
    pat: &str,
) -> Result<Vec<DefinitionSummary>> {
    let url = format!(
        "{}/{}/_apis/build/definitions?api-version=7.1",
        ctx.org_url.trim_end_matches('/'),
        ctx.project
    );

    debug!("Listing definitions: {}", url);

    let resp = client
        .get(&url)
        .basic_auth("", Some(pat))
        .send()
        .await
        .context("Failed to list pipeline definitions")?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "ADO API returned {} when listing definitions: {}",
            status,
            body
        );
    }

    let response: DefinitionListResponse = resp
        .json()
        .await
        .context("Failed to parse definitions response")?;

    Ok(response.value)
}

/// Match detected pipeline YAML files to ADO pipeline definitions.
///
/// Strategy:
/// 1. Try to match by the `yamlFilename` field in the definition's process config
/// 2. Fall back to matching by pipeline name containing the agent name
async fn match_definitions(
    client: &reqwest::Client,
    ctx: &AdoContext,
    pat: &str,
    detected: &[detect::DetectedPipeline],
) -> Result<Vec<MatchedDefinition>> {
    let definitions = list_definitions(client, ctx, pat).await?;
    info!(
        "Found {} pipeline definitions in {}/{}",
        definitions.len(),
        ctx.org_url,
        ctx.project
    );

    let mut matched = Vec::new();

    for pipeline in detected {
        let yaml_path_str = pipeline.yaml_path.to_string_lossy();
        let yaml_path_normalized = yaml_path_str.replace('\\', "/");

        // Strategy 1: Match by YAML filename in the definition
        let path_match = definitions.iter().find(|d| {
            d.process
                .as_ref()
                .and_then(|p| p.yaml_filename.as_ref())
                .is_some_and(|f| f.replace('\\', "/") == yaml_path_normalized)
        });

        if let Some(def) = path_match {
            debug!(
                "Matched '{}' to definition '{}' (id={}) by YAML path",
                yaml_path_normalized, def.name, def.id
            );
            matched.push(MatchedDefinition {
                id: def.id,
                name: def.name.clone(),
                match_method: MatchMethod::YamlPath,
                yaml_path: yaml_path_normalized.to_string(),
            });
            continue;
        }

        // Strategy 2: Match by pipeline name containing the agent name
        let agent_name = Path::new(&pipeline.source)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");

        if !agent_name.is_empty() {
            let name_match = definitions.iter().find(|d| {
                let def_name_lower = d.name.to_lowercase();
                let agent_lower = agent_name.to_lowercase().replace('-', " ");
                def_name_lower.contains(&agent_lower)
                    || def_name_lower.contains(&agent_name.to_lowercase())
            });

            if let Some(def) = name_match {
                debug!(
                    "Matched '{}' to definition '{}' (id={}) by pipeline name",
                    yaml_path_normalized, def.name, def.id
                );
                matched.push(MatchedDefinition {
                    id: def.id,
                    name: def.name.clone(),
                    match_method: MatchMethod::PipelineName,
                    yaml_path: yaml_path_normalized.to_string(),
                });
                continue;
            }
        }

        info!(
            "No ADO definition match for: {} (source: {})",
            yaml_path_normalized, pipeline.source
        );
    }

    Ok(matched)
}

/// Update the GITHUB_TOKEN pipeline variable on a definition.
///
/// Uses the same `.basic_auth("", Some(token))` pattern as the existing
/// tools in `src/tools/` (e.g., create_work_item, create_pr).
async fn update_pipeline_variable(
    client: &reqwest::Client,
    ctx: &AdoContext,
    pat: &str,
    definition_id: u64,
    variable_name: &str,
    variable_value: &str,
) -> Result<()> {
    let get_url = format!(
        "{}/{}/_apis/build/definitions/{}?api-version=7.1",
        ctx.org_url.trim_end_matches('/'),
        ctx.project,
        definition_id
    );

    debug!("Fetching definition {}: {}", definition_id, get_url);

    let resp = client
        .get(&get_url)
        .basic_auth("", Some(pat))
        .send()
        .await
        .context("Failed to get pipeline definition")?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "ADO API returned {} when getting definition {}: {}",
            status,
            definition_id,
            body
        );
    }

    let mut definition: serde_json::Value = resp
        .json()
        .await
        .context("Failed to parse definition JSON")?;

    // Ensure variables object exists
    if definition.get("variables").is_none() {
        definition["variables"] = serde_json::json!({});
    }

    // Set the variable (mark as secret since it's a token)
    definition["variables"][variable_name] = serde_json::json!({
        "value": variable_value,
        "isSecret": true,
        "allowOverride": true
    });

    let put_url = format!(
        "{}/{}/_apis/build/definitions/{}?api-version=7.1",
        ctx.org_url.trim_end_matches('/'),
        ctx.project,
        definition_id
    );

    debug!("Updating definition {}: {}", definition_id, put_url);

    let resp = client
        .put(&put_url)
        .basic_auth("", Some(pat))
        .header("Content-Type", "application/json")
        .json(&definition)
        .send()
        .await
        .context("Failed to update pipeline definition")?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "ADO API returned {} when updating definition {}: {}",
            status,
            definition_id,
            body
        );
    }

    Ok(())
}

// ==================== Command orchestration ====================

/// Run the configure command.
pub async fn run(
    token: &str,
    org: Option<&str>,
    project: Option<&str>,
    pat: Option<&str>,
    path: Option<&Path>,
    dry_run: bool,
) -> Result<()> {
    let repo_path = path
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));

    // Step 1: Detect agentic pipelines
    println!("Scanning for agentic pipelines...");
    let detected = detect::detect_pipelines(&repo_path).await?;

    if detected.is_empty() {
        println!("No agentic pipelines found. Make sure your pipelines were compiled with ado-aw >= 0.4.0.");
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

    // Step 2: Resolve ADO context from git remote, with CLI overrides
    let remote_url = get_git_remote_url(&repo_path)
        .await
        .context("Could not get git remote URL. Use --org and --project to specify manually.")?;

    info!("Git remote: {}", remote_url);

    let mut ado_ctx = parse_ado_remote(&remote_url).with_context(|| {
        format!(
            "Could not parse ADO context from remote '{}'. Use --org and --project to specify manually.",
            remote_url
        )
    })?;

    if let Some(org) = org {
        ado_ctx.org_url = org.to_string();
    }
    if let Some(project) = project {
        ado_ctx.project = project.to_string();
    }

    println!(
        "ADO context: org={}, project={}, repo={}",
        ado_ctx.org_url, ado_ctx.project, ado_ctx.repo_name
    );
    println!();

    // Step 3: Resolve PAT (same env var the existing tools check)
    let resolved_pat = match pat {
        Some(p) => p.to_string(),
        None => std::env::var("AZURE_DEVOPS_EXT_PAT").context(
            "No PAT provided. Set AZURE_DEVOPS_EXT_PAT environment variable or use --pat flag.",
        )?,
    };

    // Step 4: Match to ADO definitions
    println!("Matching to Azure DevOps pipeline definitions...");
    let client = reqwest::Client::new();
    let matched = match_definitions(&client, &ado_ctx, &resolved_pat, &detected).await?;

    if matched.is_empty() {
        println!("No matching ADO pipeline definitions found.");
        println!("Make sure your pipelines are registered in Azure DevOps and point to the detected YAML files.");
        return Ok(());
    }

    println!("Matched {} definition(s):", matched.len());
    for m in &matched {
        println!(
            "  [{}] '{}' (id={}) \u{2190} {}",
            m.match_method, m.name, m.id, m.yaml_path
        );
    }
    println!();

    // Step 5: Update GITHUB_TOKEN
    if dry_run {
        println!("Dry run \u{2014} no changes applied.");
        println!(
            "Would update GITHUB_TOKEN on {} definition(s).",
            matched.len()
        );
        return Ok(());
    }

    println!("Updating GITHUB_TOKEN on matched definitions...");
    let mut success_count = 0;
    let mut failure_count = 0;

    for m in &matched {
        match update_pipeline_variable(
            &client,
            &ado_ctx,
            &resolved_pat,
            m.id,
            "GITHUB_TOKEN",
            token,
        )
        .await
        {
            Ok(()) => {
                println!("  \u{2713} Updated '{}' (id={})", m.name, m.id);
                success_count += 1;
            }
            Err(e) => {
                eprintln!("  \u{2717} Failed to update '{}' (id={}): {}", m.name, m.id, e);
                failure_count += 1;
            }
        }
    }

    println!();
    println!("Done: {} updated, {} failed.", success_count, failure_count);

    if failure_count > 0 {
        std::process::exit(1);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ado_remote_https() {
        let url = "https://dev.azure.com/myorg/myproject/_git/myrepo";
        let ctx = parse_ado_remote(url).unwrap();
        assert_eq!(ctx.org_url, "https://dev.azure.com/myorg");
        assert_eq!(ctx.project, "myproject");
        assert_eq!(ctx.repo_name, "myrepo");
    }

    #[test]
    fn test_parse_ado_remote_https_with_git_suffix() {
        let url = "https://dev.azure.com/myorg/myproject/_git/myrepo.git";
        let ctx = parse_ado_remote(url).unwrap();
        assert_eq!(ctx.repo_name, "myrepo");
    }

    #[test]
    fn test_parse_ado_remote_ssh() {
        let url = "git@ssh.dev.azure.com:v3/myorg/myproject/myrepo";
        let ctx = parse_ado_remote(url).unwrap();
        assert_eq!(ctx.org_url, "https://dev.azure.com/myorg");
        assert_eq!(ctx.project, "myproject");
        assert_eq!(ctx.repo_name, "myrepo");
    }

    #[test]
    fn test_parse_ado_remote_legacy_visualstudio() {
        let url = "https://myorg.visualstudio.com/myproject/_git/myrepo";
        let ctx = parse_ado_remote(url).unwrap();
        assert_eq!(ctx.org_url, "https://dev.azure.com/myorg");
        assert_eq!(ctx.project, "myproject");
        assert_eq!(ctx.repo_name, "myrepo");
    }

    #[test]
    fn test_parse_ado_remote_invalid() {
        assert!(parse_ado_remote("https://github.com/user/repo").is_err());
        assert!(parse_ado_remote("not-a-url").is_err());
    }
}
