//! The `enable` CLI command.
//!
//! Registers ADO build definitions for each compiled pipeline in the
//! current repository and ensures they are `enabled`. Phase 1 of the
//! pipeline-lifecycle CLI family — see `docs/cli.md`.
//!
//! Scope (Phase 1):
//!
//! - Only operates on local fixtures discovered from `PATH` (same
//!   auto-discovery as `compile`). It does *not* enumerate ADO
//!   definitions beyond a single `list_definitions` call used for the
//!   already-exists check.
//! - Source repository must be Azure DevOps Git. GitHub-hosted source
//!   repositories are gated on a follow-up.

use anyhow::{Context, Result};
use log::debug;
use std::path::{Path, PathBuf};

use crate::ado::{
    AdoAuth, AdoContext, DefinitionSummary, create_definition, get_git_remote_url,
    get_repository_id, list_definitions, normalize_ado_yaml_path, parse_ado_remote,
    patch_queue_status, resolve_ado_context, resolve_auth, update_pipeline_variable,
};
use crate::compile;
use crate::detect;

/// Outcome of inspecting a local fixture against the ADO listing.
///
/// Pure data — no HTTP, no auth, no IO. Built by [`decide_action`] so
/// the decision logic can be exercised without touching the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// No existing definition matches; create a fresh one.
    Create,
    /// Definition exists but is not enabled; flip queueStatus to enabled.
    EnableExisting {
        id: u64,
        name: String,
        current_status: String,
    },
    /// Definition exists and is already enabled; skip.
    AlreadyEnabled { id: u64, name: String },
}

/// Sanitize a raw front-matter `name:` for use as an ADO build
/// definition display name.
///
/// ADO forbids `< > | : " * ? \` in definition names and rejects names
/// ending in `.`. This sanitizer also collapses internal whitespace
/// runs and caps the result at 255 characters (matching ADO's limit).
/// An empty result falls back to `"pipeline"` so callers always get a
/// non-empty string to POST.
///
/// Distinct from the compiler's `sanitize_pipeline_agent_name`, which
/// applies the stricter `Build.BuildNumber` rules.
pub fn sanitize_ado_display_name(raw: &str) -> String {
    const ADO_FORBIDDEN: &[char] = &['<', '>', '|', ':', '"', '*', '?', '\\'];

    let stripped: String = raw.chars().filter(|c| !ADO_FORBIDDEN.contains(c)).collect();
    let collapsed = stripped.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim_end_matches('.').to_string();
    let bounded: String = trimmed.chars().take(255).collect();
    if bounded.is_empty() {
        "pipeline".to_string()
    } else {
        bounded
    }
}

/// Compute the ADO `yamlFilename` value for a compiled pipeline.
///
/// ADO stores `yamlFilename` with a leading `/` and forward slashes;
/// we normalize both here so the value we POST round-trips exactly
/// against `normalize_ado_yaml_path` on the read side.
pub fn compute_yaml_filename(repo_relative: &Path) -> String {
    let s = repo_relative.to_string_lossy().replace('\\', "/");
    if s.starts_with('/') { s } else { format!("/{}", s) }
}

/// Pure function: decide what to do for one local fixture against a
/// snapshot of the project's ADO definitions.
///
/// Match order: yaml-path first, then exact name match. Two separate
/// passes are made so that a yaml-path match on definition B always
/// wins over a name match on definition A that appears earlier in the
/// ADO listing. Returns [`Action::Create`] when nothing matches.
pub fn decide_action(
    sanitized_name: &str,
    yaml_filename: &str,
    definitions: &[DefinitionSummary],
) -> Action {
    let target_path = normalize_ado_yaml_path(yaml_filename);

    // Pass 1: yaml-path takes precedence.
    let matched = definitions.iter().find(|def| {
        def.process
            .as_ref()
            .and_then(|p| p.yaml_filename.as_ref())
            .map(|f| normalize_ado_yaml_path(f) == target_path)
            .unwrap_or(false)
    });

    // Pass 2: fall back to exact name match.
    let matched = matched.or_else(|| definitions.iter().find(|def| def.name == sanitized_name));

    match matched {
        None => Action::Create,
        Some(def) => {
            let status = def.queue_status.clone().unwrap_or_default();
            if status == "enabled" {
                Action::AlreadyEnabled {
                    id: def.id,
                    name: def.name.clone(),
                }
            } else {
                Action::EnableExisting {
                    id: def.id,
                    name: def.name.clone(),
                    current_status: if status.is_empty() {
                        "unknown".to_string()
                    } else {
                        status
                    },
                }
            }
        }
    }
}

/// Build the JSON body for `POST /_apis/build/definitions`. Pure
/// function so we can snapshot-test the wire shape.
pub fn build_create_body(
    name: &str,
    folder: &str,
    repo_id: &str,
    repo_name: &str,
    default_branch: &str,
    yaml_filename: &str,
) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "path": folder,
        "type": "build",
        "queueStatus": "enabled",
        "repository": {
            "id": repo_id,
            "type": "TfsGit",
            "name": repo_name,
            "defaultBranch": default_branch,
            "properties": { "reportBuildStatus": "true" }
        },
        "process": {
            "type": 2,
            "yamlFilename": yaml_filename
        },
        "triggers": []
    })
}

/// CLI options for [`run`]. Bundled into a struct so we don't grow a
/// 10-positional-argument signature as Phase 1 evolves.
pub struct EnableOptions<'a> {
    pub org: Option<&'a str>,
    pub project: Option<&'a str>,
    pub pat: Option<&'a str>,
    pub path: Option<&'a Path>,
    pub folder: &'a str,
    pub default_branch: &'a str,
    pub dry_run: bool,
    pub also_set_token: bool,
    pub token: Option<&'a str>,
}

/// Run the `enable` command.
pub async fn run(opts: EnableOptions<'_>) -> Result<()> {
    let repo_path: PathBuf = match opts.path {
        Some(p) => tokio::fs::canonicalize(p)
            .await
            .with_context(|| format!("Could not resolve path: {}", p.display()))?,
        None => tokio::fs::canonicalize(".")
            .await
            .context("Could not resolve current directory")?,
    };

    // GitHub-source guard: Phase 1 only supports ADO Git source
    // repositories. We use parse_ado_remote success on the raw git
    // remote URL as the gate — if we can't parse it as ADO we don't
    // have a `repository.name` to put in the POST body.
    let remote_url = get_git_remote_url(&repo_path).await.with_context(|| {
        format!(
            "Could not read the git remote 'origin' from {}. \
             `ado-aw enable` requires an Azure DevOps Git remote.",
            repo_path.display()
        )
    })?;
    let remote_ctx = match parse_ado_remote(&remote_url) {
        Ok(ctx) => ctx,
        Err(_) => anyhow::bail!(
            "This command requires an Azure DevOps Git remote.\n\
             The current remote is {}.\n\
             Phase 1 of `ado-aw enable` does not yet support GitHub-hosted source \
             repos. Follow https://github.com/githubnext/ado-aw/issues for the \
             GitHub-source follow-up.",
            remote_url
        ),
    };

    // Skip interactive token resolution on dry-run: --also-set-token is
    // silently suppressed in that path anyway, so prompting the operator
    // for a credential they will never use is wrong UX.
    let github_token = if opts.dry_run {
        None
    } else {
        resolve_token_arg(opts.also_set_token, opts.token)?
    };
    let auth = resolve_auth(opts.pat).await?;
    let ado_ctx = resolve_ado_context(&repo_path, opts.org, opts.project).await?;

    println!(
        "ADO context: org={}, project={}, repo={}",
        ado_ctx.org_url, ado_ctx.project, remote_ctx.repo_name
    );
    println!();

    println!("Scanning for agentic pipelines...");
    let detected = detect::detect_pipelines(&repo_path).await?;
    if detected.is_empty() {
        println!(
            "No agentic pipelines found. Make sure your pipelines were compiled with the latest ado-aw."
        );
        return Ok(());
    }
    println!("Found {} agentic pipeline(s).", detected.len());
    println!();

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("Failed to create HTTP client")?;

    let definitions = list_definitions(&client, &ado_ctx, &auth).await?;

    // Resolve the repository GUID once. Dry-run skips this — the
    // POST body printout uses a placeholder GUID so operators still
    // see the full body shape.
    let repo_id = if opts.dry_run {
        String::from("<repo-id>")
    } else {
        get_repository_id(&client, &ado_ctx, &auth, &remote_ctx.repo_name)
            .await
            .with_context(|| {
                format!(
                    "Failed to look up ADO repository '{}'",
                    remote_ctx.repo_name
                )
            })?
    };

    let mut success = 0usize;
    let mut failure = 0usize;
    let mut newly_created_ids: Vec<u64> = Vec::new();

    for pipeline in &detected {
        let source_path = repo_path.join(&pipeline.source);
        let result = process_one(
            &client,
            &ado_ctx,
            &auth,
            &definitions,
            &pipeline.yaml_path,
            &source_path,
            &repo_id,
            &remote_ctx.repo_name,
            opts.folder,
            opts.default_branch,
            opts.dry_run,
        )
        .await;
        match result {
            Ok(Some(new_id)) => {
                newly_created_ids.push(new_id);
                success += 1;
            }
            Ok(None) => success += 1,
            Err(e) => {
                eprintln!("✗ failed: {}: {:#}", pipeline.source, e);
                failure += 1;
            }
        }
    }

    if opts.also_set_token && !opts.dry_run && !newly_created_ids.is_empty() {
        let Some(token) = github_token.as_deref() else {
            unreachable!("resolve_token_arg guarantees Some when also_set_token is true");
        };
        println!();
        println!(
            "Setting GITHUB_TOKEN on {} newly-created definition(s)...",
            newly_created_ids.len()
        );
        for id in &newly_created_ids {
            match update_pipeline_variable(&client, &ado_ctx, &auth, *id, "GITHUB_TOKEN", token)
                .await
            {
                Ok(()) => println!("  ✓ set GITHUB_TOKEN on definition {}", id),
                Err(e) => {
                    eprintln!("  ✗ failed to set GITHUB_TOKEN on {}: {:#}", id, e);
                    failure += 1;
                }
            }
        }
    }

    println!();
    println!("Done: {} succeeded, {} failed.", success, failure);
    if failure > 0 {
        anyhow::bail!("{} fixture(s) failed", failure);
    }
    Ok(())
}

/// Validates the `--token` / `--also-set-token` pair and resolves the
/// effective token value when `--also-set-token` is set.
fn resolve_token_arg(also_set_token: bool, token: Option<&str>) -> Result<Option<String>> {
    if !also_set_token {
        if token.is_some() {
            anyhow::bail!("--token requires --also-set-token");
        }
        return Ok(None);
    }
    if let Some(t) = token {
        return Ok(Some(t.to_string()));
    }
    if let Ok(env) = std::env::var("GITHUB_TOKEN")
        && !env.is_empty()
    {
        return Ok(Some(env));
    }
    let prompted = inquire::Password::new("Enter the GITHUB_TOKEN to set on newly-created definitions:")
        .without_confirmation()
        .prompt()
        .context("Failed to read token from interactive prompt")?;
    Ok(Some(prompted))
}

#[allow(clippy::too_many_arguments)]
async fn process_one(
    client: &reqwest::Client,
    ado_ctx: &AdoContext,
    auth: &AdoAuth,
    definitions: &[DefinitionSummary],
    yaml_path: &Path,
    source_path: &Path,
    repo_id: &str,
    repo_name: &str,
    folder: &str,
    default_branch: &str,
    dry_run: bool,
) -> Result<Option<u64>> {
    let content = tokio::fs::read_to_string(source_path)
        .await
        .with_context(|| format!("Failed to read source: {}", source_path.display()))?;
    let parsed = compile::parse_markdown_detailed(&content)
        .with_context(|| format!("Failed to parse front matter: {}", source_path.display()))?;
    let sanitized = sanitize_ado_display_name(&parsed.front_matter.name);

    let yaml_filename = compute_yaml_filename(yaml_path);

    let action = decide_action(&sanitized, &yaml_filename, definitions);
    debug!(
        "fixture {}: sanitized_name='{}' yaml='{}' action={:?}",
        source_path.display(),
        sanitized,
        yaml_filename,
        action
    );

    match action {
        Action::AlreadyEnabled { id, name } => {
            println!("↻ already enabled: {} (id={})", name, id);
            Ok(None)
        }
        Action::EnableExisting {
            id,
            name,
            current_status,
        } => {
            if dry_run {
                println!(
                    "[dry-run] ▶ would enable: {} (id={}, current={})",
                    name, id, current_status
                );
                return Ok(None);
            }
            patch_queue_status(client, ado_ctx, auth, id, "enabled")
                .await
                .with_context(|| format!("Failed to enable definition {}", id))?;
            println!("▶ enabled: {} (id={}, was {})", name, id, current_status);
            Ok(None)
        }
        Action::Create => {
            let body = build_create_body(
                &sanitized,
                folder,
                repo_id,
                repo_name,
                default_branch,
                &yaml_filename,
            );
            if dry_run {
                let pretty = serde_json::to_string_pretty(&body).unwrap_or_default();
                println!(
                    "[dry-run] ✓ would create: {} ← {}",
                    sanitized, yaml_filename
                );
                println!("{}", pretty);
                return Ok(None);
            }
            let new_id = create_definition(client, ado_ctx, auth, &body)
                .await
                .with_context(|| format!("Failed to create definition for {}", sanitized))?;
            println!(
                "✓ registered + enabled: {} (id={}) ← {}",
                sanitized, new_id, yaml_filename
            );
            Ok(Some(new_id))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ado::ProcessInfo;

    fn def(id: u64, name: &str, yaml: Option<&str>, status: Option<&str>) -> DefinitionSummary {
        DefinitionSummary {
            id,
            name: name.to_string(),
            process: yaml.map(|y| ProcessInfo {
                yaml_filename: Some(y.to_string()),
            }),
            queue_status: status.map(str::to_string),
        }
    }

    // ============ sanitize_ado_display_name ============

    #[test]
    fn sanitize_strips_forbidden_chars() {
        assert_eq!(
            sanitize_ado_display_name("pipe<line>|name:foo*?\\bar\""),
            "pipelinenamefoobar"
        );
    }

    #[test]
    fn sanitize_collapses_internal_whitespace() {
        assert_eq!(
            sanitize_ado_display_name("daily   smoke    noop"),
            "daily smoke noop"
        );
    }

    #[test]
    fn sanitize_trims_trailing_dot() {
        assert_eq!(sanitize_ado_display_name("name..."), "name");
        assert_eq!(sanitize_ado_display_name("name."), "name");
        assert_eq!(sanitize_ado_display_name("name.with.dots"), "name.with.dots");
    }

    #[test]
    fn sanitize_caps_length_at_255() {
        let raw = "a".repeat(300);
        let out = sanitize_ado_display_name(&raw);
        assert_eq!(out.chars().count(), 255);
    }

    #[test]
    fn sanitize_falls_back_to_pipeline_when_empty() {
        assert_eq!(sanitize_ado_display_name("<<<>>>"), "pipeline");
        assert_eq!(sanitize_ado_display_name(""), "pipeline");
        assert_eq!(sanitize_ado_display_name("   "), "pipeline");
        // All-dot after stripping should also fall back.
        assert_eq!(sanitize_ado_display_name("..."), "pipeline");
    }

    #[test]
    fn sanitize_preserves_safe_content() {
        assert_eq!(
            sanitize_ado_display_name("Daily safe-output smoke: noop"),
            "Daily safe-output smoke noop"
        );
    }

    // ============ compute_yaml_filename ============

    #[test]
    fn compute_yaml_filename_adds_leading_slash() {
        assert_eq!(
            compute_yaml_filename(Path::new("tests/safe-outputs/noop.lock.yml")),
            "/tests/safe-outputs/noop.lock.yml"
        );
    }

    #[test]
    fn compute_yaml_filename_preserves_leading_slash() {
        assert_eq!(
            compute_yaml_filename(Path::new("/already/rooted.yml")),
            "/already/rooted.yml"
        );
    }

    #[test]
    fn compute_yaml_filename_normalizes_backslashes() {
        assert_eq!(
            compute_yaml_filename(Path::new(r"tests\safe-outputs\noop.lock.yml")),
            "/tests/safe-outputs/noop.lock.yml"
        );
    }

    // ============ decide_action ============

    #[test]
    fn decide_action_create_when_no_match() {
        let defs = vec![def(1, "other", Some("/other.yml"), Some("enabled"))];
        let action = decide_action("noop", "/tests/noop.yml", &defs);
        assert_eq!(action, Action::Create);
    }

    #[test]
    fn decide_action_matches_by_yaml_path_even_when_name_differs() {
        let defs = vec![def(
            7,
            "Some old name",
            Some("/tests/noop.lock.yml"),
            Some("enabled"),
        )];
        let action = decide_action("My noop", "/tests/noop.lock.yml", &defs);
        assert_eq!(
            action,
            Action::AlreadyEnabled {
                id: 7,
                name: "Some old name".to_string()
            }
        );
    }

    #[test]
    fn decide_action_matches_by_name_when_yaml_missing() {
        let defs = vec![def(42, "Daily noop", None, Some("enabled"))];
        let action = decide_action("Daily noop", "/whatever.yml", &defs);
        assert_eq!(
            action,
            Action::AlreadyEnabled {
                id: 42,
                name: "Daily noop".to_string()
            }
        );
    }

    #[test]
    fn decide_action_enable_existing_when_disabled() {
        let defs = vec![def(
            10,
            "Daily noop",
            Some("/tests/noop.lock.yml"),
            Some("disabled"),
        )];
        let action = decide_action("Daily noop", "/tests/noop.lock.yml", &defs);
        assert_eq!(
            action,
            Action::EnableExisting {
                id: 10,
                name: "Daily noop".to_string(),
                current_status: "disabled".to_string()
            }
        );
    }

    #[test]
    fn decide_action_enable_existing_when_paused() {
        let defs = vec![def(
            11,
            "Daily noop",
            Some("/tests/noop.lock.yml"),
            Some("paused"),
        )];
        let action = decide_action("Daily noop", "/tests/noop.lock.yml", &defs);
        assert_eq!(
            action,
            Action::EnableExisting {
                id: 11,
                name: "Daily noop".to_string(),
                current_status: "paused".to_string()
            }
        );
    }

    #[test]
    fn decide_action_enable_existing_when_status_missing() {
        // Old/cached responses may omit queueStatus — treat as not-enabled.
        let defs = vec![def(12, "Daily noop", Some("/tests/noop.lock.yml"), None)];
        let action = decide_action("Daily noop", "/tests/noop.lock.yml", &defs);
        assert_eq!(
            action,
            Action::EnableExisting {
                id: 12,
                name: "Daily noop".to_string(),
                current_status: "unknown".to_string()
            }
        );
    }

    #[test]
    fn decide_action_yaml_match_handles_backslashes_and_leading_slash() {
        // ADO sometimes returns yamlFilename with backslashes or without
        // the leading slash; normalize_ado_yaml_path handles both, and
        // decide_action must pick this up.
        let defs = vec![def(
            5,
            "n",
            Some(r"\tests\safe-outputs\noop.lock.yml"),
            Some("enabled"),
        )];
        let action = decide_action("n", "/tests/safe-outputs/noop.lock.yml", &defs);
        assert_eq!(
            action,
            Action::AlreadyEnabled {
                id: 5,
                name: "n".to_string()
            }
        );
    }

    #[test]
    fn decide_action_yaml_path_wins_over_earlier_name_match() {
        // Definition A appears first and matches by name.
        // Definition B appears second and matches by yaml path.
        // yaml-path must win (definition B), even though A is earlier in the slice.
        let defs = vec![
            def(1, "My Pipeline", None, Some("enabled")),           // name match, no path
            def(2, "Old Name", Some("/pipelines/agent.yml"), Some("disabled")), // path match, different name
        ];
        let action = decide_action("My Pipeline", "/pipelines/agent.yml", &defs);
        // Should pick definition 2 (path match) not 1 (name match).
        assert_eq!(
            action,
            Action::EnableExisting {
                id: 2,
                name: "Old Name".to_string(),
                current_status: "disabled".to_string()
            }
        );
    }

    // ============ build_create_body ============

    #[test]
    fn build_create_body_matches_expected_shape() {
        let body = build_create_body(
            "Daily smoke",
            "\\smoke",
            "abc-123",
            "myrepo",
            "refs/heads/main",
            "/tests/noop.lock.yml",
        );
        let expected = serde_json::json!({
            "name": "Daily smoke",
            "path": "\\smoke",
            "type": "build",
            "queueStatus": "enabled",
            "repository": {
                "id": "abc-123",
                "type": "TfsGit",
                "name": "myrepo",
                "defaultBranch": "refs/heads/main",
                "properties": { "reportBuildStatus": "true" }
            },
            "process": {
                "type": 2,
                "yamlFilename": "/tests/noop.lock.yml"
            },
            "triggers": []
        });
        assert_eq!(body, expected);
    }

    // ============ resolve_token_arg ============

    #[test]
    fn resolve_token_arg_rejects_token_without_also_set() {
        let err = resolve_token_arg(false, Some("x")).unwrap_err();
        assert!(err.to_string().contains("--token requires --also-set-token"));
    }

    #[test]
    fn resolve_token_arg_returns_none_when_disabled() {
        let v = resolve_token_arg(false, None).unwrap();
        assert!(v.is_none());
    }

    #[test]
    fn resolve_token_arg_takes_explicit_token() {
        let v = resolve_token_arg(true, Some("explicit")).unwrap();
        assert_eq!(v.as_deref(), Some("explicit"));
    }
}
