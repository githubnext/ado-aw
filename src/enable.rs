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
    AdoAuth, AdoContext, DefinitionSummary, RepoProvider, RepoSource, create_definition,
    get_git_remote_url, get_repository_id, list_definitions, normalize_ado_yaml_path,
    parse_git_remote, patch_queue_status, resolve_ado_context, resolve_auth,
    resolve_service_connection_id, update_pipeline_variable,
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
    // Trim trailing dots before *and* after capping: capping a string like
    // "aaa....(254 a's)...suffix" at 255 chars can re-introduce a trailing dot
    // if the 255th character is '.'.
    let trimmed = collapsed.trim_end_matches('.');
    let bounded: String = trimmed.chars().take(255).collect();
    let bounded = bounded.trim_end_matches('.');
    if bounded.is_empty() {
        "pipeline".to_string()
    } else {
        bounded.to_string()
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

/// How `build_create_body` should describe the backing repository on
/// the create-definition POST body. The two variants emit different
/// shapes for the `repository:` object.
#[derive(Debug, Clone, Copy)]
pub enum RepositoryRef<'a> {
    /// Azure DevOps Git source. `repo_id` is the repository GUID
    /// returned by `get_repository_id`; `repo_name` is the bare repo
    /// name (e.g. `myrepo`).
    AdoGit {
        repo_id: &'a str,
        repo_name: &'a str,
    },
    /// GitHub source. `full_name` is `owner/repo` (the form ADO
    /// expects in `repository.name`); `connected_service_id` is the
    /// GUID of a project-level GitHub service connection.
    Github {
        full_name: &'a str,
        connected_service_id: &'a str,
    },
}

/// Build the JSON body for `POST /_apis/build/definitions`. Pure
/// function so we can snapshot-test the wire shape.
pub fn build_create_body(
    name: &str,
    folder: &str,
    repo: RepositoryRef<'_>,
    default_branch: &str,
    yaml_filename: &str,
) -> serde_json::Value {
    let repository = match repo {
        RepositoryRef::AdoGit { repo_id, repo_name } => serde_json::json!({
            "id": repo_id,
            "type": "TfsGit",
            "name": repo_name,
            "defaultBranch": default_branch,
            "properties": { "reportBuildStatus": "true" }
        }),
        RepositoryRef::Github {
            full_name,
            connected_service_id,
        } => serde_json::json!({
            "type": "GitHub",
            "name": full_name,
            "url": format!("https://github.com/{}.git", full_name),
            "defaultBranch": default_branch,
            "properties": {
                "connectedServiceId": connected_service_id,
                "reportBuildStatus": "true"
            }
        }),
    };

    serde_json::json!({
        "name": name,
        "path": folder,
        "type": "build",
        "queueStatus": "enabled",
        "repository": repository,
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
    /// GitHub service-connection name or GUID. Required when source
    /// is GitHub; clap-level error when source is ADO Git.
    pub service_connection: Option<&'a str>,
    /// Source repository override as `owner/repo`. Only honoured for
    /// GitHub source (mostly an escape hatch when the local git
    /// remote can't be parsed and the operator wants to register a
    /// pipeline backed by a specific GitHub repo).
    pub repository_name: Option<&'a str>,
}

/// Resolved source identity for an `enable` invocation.
///
/// Captures the autodetect outcome from the git remote plus operator
/// overrides. Drives the create-definition body shape and any
/// provider-specific REST lookups (repo GUID for ADO; service
/// connection GUID for GitHub).
#[derive(Debug)]
enum ResolvedSource {
    AdoGit { source: RepoSource },
    Github { source: RepoSource, service_connection: String },
}

/// Apply the autodetect rule to a parsed git remote (or its absence)
/// plus operator-supplied overrides.
///
/// Rules (mirrors `docs/cli.md`):
/// - ADO remote: ignore `--repository-name`; reject
///   `--service-connection` with a clear message.
/// - GitHub remote: require `--service-connection`; allow
///   `--repository-name` to override the auto-detected owner/repo.
/// - Neither: require both `--repository-name owner/repo` and
///   `--service-connection`; treat as GitHub source.
fn resolve_source(
    parsed_remote: Result<RepoSource>,
    remote_url: Option<&str>,
    repository_name_override: Option<&str>,
    service_connection: Option<&str>,
) -> Result<ResolvedSource> {
    match parsed_remote {
        Ok(source) if source.provider == RepoProvider::AdoGit => {
            if service_connection.is_some() {
                anyhow::bail!(
                    "--service-connection is only valid when the source repository is on GitHub. \
                     The current git remote ({}) is an Azure DevOps Git repository.",
                    remote_url.unwrap_or("?")
                );
            }
            if repository_name_override.is_some() {
                anyhow::bail!(
                    "--repository-name is only valid when the source repository is on GitHub. \
                     The current git remote ({}) is an Azure DevOps Git repository.",
                    remote_url.unwrap_or("?")
                );
            }
            Ok(ResolvedSource::AdoGit { source })
        }
        Ok(mut source) => {
            // GitHub remote.
            let sc = service_connection.ok_or_else(|| {
                anyhow::anyhow!(
                    "--service-connection <name-or-guid> is required when the source repository is on GitHub. \
                     Create a GitHub service connection in the target ADO project (Project settings → \
                     Service connections → GitHub) and pass its name or GUID."
                )
            })?;
            if let Some(over) = repository_name_override {
                let (owner, repo) = split_owner_repo_arg(over)?;
                source.owner = owner;
                source.repo = repo;
            }
            Ok(ResolvedSource::Github {
                source,
                service_connection: sc.to_string(),
            })
        }
        Err(_) => {
            // Remote is missing or unparseable; require explicit
            // flags to treat as GitHub source.
            let sc = service_connection.ok_or_else(|| {
                anyhow::anyhow!(
                    "Could not infer source repository from the git remote. \
                     For a GitHub-source pipeline, pass --repository-name owner/repo and --service-connection."
                )
            })?;
            let name = repository_name_override.ok_or_else(|| {
                anyhow::anyhow!(
                    "Could not infer source repository from the git remote. \
                     Pass --repository-name owner/repo to identify the GitHub source repo."
                )
            })?;
            let (owner, repo) = split_owner_repo_arg(name)?;
            Ok(ResolvedSource::Github {
                source: RepoSource {
                    provider: RepoProvider::Github,
                    owner,
                    repo,
                    project: None,
                },
                service_connection: sc.to_string(),
            })
        }
    }
}

/// Parse a CLI-supplied `owner/repo` argument with a useful error
/// message when the shape is wrong.
fn split_owner_repo_arg(value: &str) -> Result<(String, String)> {
    let trimmed = value.trim();
    let parts: Vec<&str> = trimmed.splitn(2, '/').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        anyhow::bail!(
            "--repository-name must be in the form 'owner/repo' (got: '{}')",
            value
        );
    }
    Ok((parts[0].trim().to_string(), parts[1].trim().to_string()))
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

    // Autodetect source identity from the git remote. The remote is
    // best-effort — `resolve_source` decides whether the absence is
    // fatal based on which CLI overrides the operator passed.
    let remote_url = get_git_remote_url(&repo_path).await.ok();
    let parsed = match remote_url.as_deref() {
        Some(url) => parse_git_remote(url),
        None => Err(anyhow::anyhow!(
            "no git remote 'origin' configured in {}",
            repo_path.display()
        )),
    };
    let resolved = resolve_source(
        parsed,
        remote_url.as_deref(),
        opts.repository_name,
        opts.service_connection,
    )?;

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

    match &resolved {
        ResolvedSource::AdoGit { source } => {
            println!(
                "ADO context: org={}, project={}, repo={} (ADO Git source)",
                ado_ctx.org_url, ado_ctx.project, source.repo
            );
        }
        ResolvedSource::Github {
            source,
            service_connection,
        } => {
            println!(
                "ADO context: org={}, project={}",
                ado_ctx.org_url, ado_ctx.project
            );
            println!(
                "GitHub source: {}/{} (service connection: {})",
                source.owner, source.repo, service_connection
            );
        }
    }
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

    // Resolve provider-specific identifiers once per invocation.
    //
    // - ADO Git source: look up the repository GUID (required by the
    //   `repository.id` field in the POST body).
    // - GitHub source: resolve the service-connection name → GUID
    //   (required by `repository.properties.connectedServiceId`).
    //
    // Dry-run skips the network calls; the POST body printout uses
    // placeholders so operators still see the full body shape.
    let (repo_id, service_conn_id) = match &resolved {
        ResolvedSource::AdoGit { source } => {
            let id = if opts.dry_run {
                String::from("<repo-id>")
            } else {
                get_repository_id(&client, &ado_ctx, &auth, &source.repo)
                    .await
                    .with_context(|| {
                        format!("Failed to look up ADO repository '{}'", source.repo)
                    })?
            };
            (id, String::new())
        }
        ResolvedSource::Github {
            service_connection, ..
        } => {
            let id = if opts.dry_run {
                String::from("<service-connection-id>")
            } else {
                resolve_service_connection_id(
                    &client,
                    &ado_ctx,
                    &auth,
                    service_connection,
                )
                .await
                .with_context(|| {
                    format!(
                        "Failed to resolve GitHub service connection '{}' in project '{}'",
                        service_connection, ado_ctx.project
                    )
                })?
            };
            (String::new(), id)
        }
    };

    let mut success = 0usize;
    let mut failure = 0usize;
    let mut newly_created_ids: Vec<u64> = Vec::new();

    // Compose the GitHub `owner/repo` once if applicable — it's
    // identical for every fixture in this invocation.
    let github_full_name = match &resolved {
        ResolvedSource::Github { source, .. } => Some(format!("{}/{}", source.owner, source.repo)),
        _ => None,
    };

    for pipeline in &detected {
        let source_path = repo_path.join(&pipeline.source);
        let repo_ref = match &resolved {
            ResolvedSource::AdoGit { source } => RepositoryRef::AdoGit {
                repo_id: &repo_id,
                repo_name: &source.repo,
            },
            ResolvedSource::Github { .. } => RepositoryRef::Github {
                full_name: github_full_name.as_deref().unwrap_or(""),
                connected_service_id: &service_conn_id,
            },
        };
        let result = process_one(
            &client,
            &ado_ctx,
            &auth,
            &definitions,
            &pipeline.yaml_path,
            &source_path,
            repo_ref,
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
    } else if opts.also_set_token && opts.dry_run && !newly_created_ids.is_empty() {
        println!();
        println!(
            "[dry-run] would set GITHUB_TOKEN on {} newly-created definition(s)",
            newly_created_ids.len()
        );
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
    repo: RepositoryRef<'_>,
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
                repo,
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
            path: None,
            repository: None,
            revision: None,
        }
    }

    fn ado_source() -> RepoSource {
        RepoSource {
            provider: RepoProvider::AdoGit,
            owner: "myorg".to_string(),
            repo: "myrepo".to_string(),
            project: Some("MyProject".to_string()),
        }
    }

    fn github_source() -> RepoSource {
        RepoSource {
            provider: RepoProvider::Github,
            owner: "githubnext".to_string(),
            repo: "ado-aw".to_string(),
            project: None,
        }
    }

    // ============ split_owner_repo_arg ============

    #[test]
    fn split_owner_repo_arg_accepts_owner_slash_repo() {
        assert_eq!(
            split_owner_repo_arg("githubnext/ado-aw").unwrap(),
            ("githubnext".to_string(), "ado-aw".to_string())
        );
    }

    #[test]
    fn split_owner_repo_arg_rejects_missing_slash() {
        let err = split_owner_repo_arg("just-a-name").unwrap_err().to_string();
        assert!(err.contains("--repository-name"));
        assert!(err.contains("owner/repo"));
    }

    #[test]
    fn split_owner_repo_arg_rejects_empty_halves() {
        assert!(split_owner_repo_arg("/repo").is_err());
        assert!(split_owner_repo_arg("owner/").is_err());
        assert!(split_owner_repo_arg("/").is_err());
    }

    // ============ resolve_source ============

    #[test]
    fn resolve_source_ado_remote_no_overrides() {
        let resolved = resolve_source(Ok(ado_source()), Some("https://..."), None, None).unwrap();
        assert!(matches!(resolved, ResolvedSource::AdoGit { .. }));
    }

    #[test]
    fn resolve_source_ado_remote_rejects_service_connection() {
        let err = resolve_source(
            Ok(ado_source()),
            Some("https://dev.azure.com/myorg/MyProject/_git/myrepo"),
            None,
            Some("ado-aw-github"),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("--service-connection is only valid"));
        assert!(err.contains("GitHub"));
    }

    #[test]
    fn resolve_source_ado_remote_rejects_repository_name() {
        let err = resolve_source(
            Ok(ado_source()),
            Some("https://dev.azure.com/myorg/MyProject/_git/myrepo"),
            Some("owner/repo"),
            None,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("--repository-name is only valid"));
    }

    #[test]
    fn resolve_source_github_remote_requires_service_connection() {
        let err = resolve_source(Ok(github_source()), Some("https://..."), None, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("--service-connection"));
        assert!(err.contains("required"));
    }

    #[test]
    fn resolve_source_github_remote_with_service_connection() {
        let resolved = resolve_source(
            Ok(github_source()),
            Some("https://..."),
            None,
            Some("ado-aw-github"),
        )
        .unwrap();
        match resolved {
            ResolvedSource::Github {
                source,
                service_connection,
            } => {
                assert_eq!(source.owner, "githubnext");
                assert_eq!(source.repo, "ado-aw");
                assert_eq!(service_connection, "ado-aw-github");
            }
            _ => panic!("expected Github"),
        }
    }

    #[test]
    fn resolve_source_github_remote_repository_name_override() {
        let resolved = resolve_source(
            Ok(github_source()),
            Some("https://..."),
            Some("other-org/other-repo"),
            Some("conn"),
        )
        .unwrap();
        match resolved {
            ResolvedSource::Github { source, .. } => {
                assert_eq!(source.owner, "other-org");
                assert_eq!(source.repo, "other-repo");
            }
            _ => panic!("expected Github"),
        }
    }

    #[test]
    fn resolve_source_no_remote_requires_both_overrides() {
        let parsed_err = || Err(anyhow::anyhow!("no remote"));
        let err = resolve_source(parsed_err(), None, None, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("--service-connection"));

        let err = resolve_source(parsed_err(), None, None, Some("conn"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("--repository-name"));

        let err = resolve_source(parsed_err(), None, Some("owner/repo"), None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("--service-connection"));
    }

    #[test]
    fn resolve_source_no_remote_with_both_overrides_treats_as_github() {
        let resolved = resolve_source(
            Err(anyhow::anyhow!("no remote")),
            None,
            Some("githubnext/ado-aw"),
            Some("conn-guid"),
        )
        .unwrap();
        match resolved {
            ResolvedSource::Github {
                source,
                service_connection,
            } => {
                assert_eq!(source.owner, "githubnext");
                assert_eq!(source.repo, "ado-aw");
                assert_eq!(service_connection, "conn-guid");
            }
            _ => panic!("expected Github"),
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
    fn sanitize_truncation_does_not_leave_trailing_dot() {
        // 254 'a' chars followed by ".extra" — the first 255 chars are
        // "a".repeat(254) + "." which must be trimmed back to 254 chars.
        let raw = "a".repeat(254) + ".extra";
        let out = sanitize_ado_display_name(&raw);
        assert!(!out.ends_with('.'), "result must not end with '.'");
        assert_eq!(out, "a".repeat(254));
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
    fn build_create_body_ado_git_shape() {
        let body = build_create_body(
            "Daily smoke",
            "\\smoke",
            RepositoryRef::AdoGit {
                repo_id: "abc-123",
                repo_name: "myrepo",
            },
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

    #[test]
    fn build_create_body_github_shape() {
        let body = build_create_body(
            "Daily smoke noop",
            "\\smoke",
            RepositoryRef::Github {
                full_name: "githubnext/ado-aw",
                connected_service_id: "11111111-2222-3333-4444-555555555555",
            },
            "refs/heads/main",
            "/tests/safe-outputs/noop.lock.yml",
        );
        let expected = serde_json::json!({
            "name": "Daily smoke noop",
            "path": "\\smoke",
            "type": "build",
            "queueStatus": "enabled",
            "repository": {
                "type": "GitHub",
                "name": "githubnext/ado-aw",
                "url": "https://github.com/githubnext/ado-aw.git",
                "defaultBranch": "refs/heads/main",
                "properties": {
                    "connectedServiceId": "11111111-2222-3333-4444-555555555555",
                    "reportBuildStatus": "true"
                }
            },
            "process": {
                "type": 2,
                "yamlFilename": "/tests/safe-outputs/noop.lock.yml"
            },
            "triggers": []
        });
        assert_eq!(body, expected);
    }

    #[test]
    fn build_create_body_github_omits_repo_id() {
        // GitHub-source bodies must not carry a `repository.id` field
        // — ADO uses `repository.name = owner/repo` to identify the
        // GitHub repo, and the connectedServiceId tells it how to
        // authenticate.
        let body = build_create_body(
            "n",
            "\\",
            RepositoryRef::Github {
                full_name: "owner/repo",
                connected_service_id: "guid",
            },
            "refs/heads/main",
            "/x.yml",
        );
        assert!(
            body.get("repository")
                .and_then(|r| r.get("id"))
                .is_none(),
            "GitHub-source body must not include repository.id"
        );
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
