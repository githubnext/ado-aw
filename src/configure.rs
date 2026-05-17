//! The `configure` CLI command.
//!
//! Detects agentic pipelines in a local repository and updates the
//! `GITHUB_TOKEN` pipeline variable on their corresponding Azure DevOps
//! build definitions.
//!
//! Note: this command is being renamed to `secrets set GITHUB_TOKEN` as
//! part of the Phase 1 CLI overhaul. The current entry point remains the
//! orchestration shim below; all shared ADO REST logic lives in
//! [`crate::ado`].

use anyhow::{Context, Result};
use std::path::Path;

use crate::ado::{
    AdoAuth, AdoContext, MatchedDefinition, resolve_ado_context, resolve_auth,
    resolve_definitions, update_pipeline_variable,
};

/// Resolves the GitHub token from the CLI flag or an interactive prompt.
fn resolve_token(token: Option<&str>) -> Result<String> {
    match token {
        Some(t) => Ok(t.to_string()),
        None => inquire::Password::new("Enter the new GITHUB_TOKEN:")
            .without_confirmation()
            .prompt()
            .context("Failed to read token from interactive prompt"),
    }
}

/// Updates the `GITHUB_TOKEN` variable on every matched pipeline
/// definition and reports per-definition success/failure.
async fn apply_token_updates(
    client: &reqwest::Client,
    ado_ctx: &AdoContext,
    auth: &AdoAuth,
    matched: &[MatchedDefinition],
    token: &str,
) -> Result<()> {
    println!("Updating GITHUB_TOKEN on matched definitions...");
    let mut success_count = 0;
    let mut failure_count = 0;

    for m in matched {
        match update_pipeline_variable(client, ado_ctx, auth, m.id, "GITHUB_TOKEN", token).await {
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
        anyhow::bail!("{} definition(s) failed to update", failure_count);
    }

    Ok(())
}

/// Run the configure command.
pub async fn run(
    token: Option<&str>,
    org: Option<&str>,
    project: Option<&str>,
    pat: Option<&str>,
    path: Option<&Path>,
    dry_run: bool,
    definition_ids: Option<&[u64]>,
) -> Result<()> {
    let repo_path = match path {
        Some(p) => tokio::fs::canonicalize(p)
            .await
            .with_context(|| format!("Could not resolve path: {}", p.display()))?,
        None => tokio::fs::canonicalize(".")
            .await
            .context("Could not resolve current directory")?,
    };

    let token = resolve_token(token)?;
    let auth = resolve_auth(pat).await?;
    let ado_ctx = resolve_ado_context(&repo_path, org, project).await?;

    println!(
        "ADO context: org={}, project={}{}",
        ado_ctx.org_url,
        ado_ctx.project,
        if ado_ctx.repo_name.is_empty() {
            String::new()
        } else {
            format!(", repo={}", ado_ctx.repo_name)
        }
    );
    println!();

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("Failed to create HTTP client")?;

    let Some(matched) =
        resolve_definitions(&client, &ado_ctx, &auth, definition_ids, &repo_path).await?
    else {
        return Ok(());
    };

    if matched.is_empty() {
        println!("No matching ADO pipeline definitions found.");
        println!("Make sure your pipelines are registered in Azure DevOps and point to the detected YAML files.");
        return Ok(());
    }

    println!("{} definition(s) to update:", matched.len());
    for m in &matched {
        if m.yaml_path.is_empty() {
            println!("  [{}] '{}' (id={})", m.match_method, m.name, m.id);
        } else {
            println!(
                "  [{}] '{}' (id={}) \u{2190} {}",
                m.match_method, m.name, m.id, m.yaml_path
            );
        }
    }
    println!();

    if dry_run {
        println!("Dry run \u{2014} no changes applied.");
        println!(
            "Would update GITHUB_TOKEN on {} definition(s).",
            matched.len()
        );
        return Ok(());
    }

    apply_token_updates(&client, &ado_ctx, &auth, &matched, &token).await
}
