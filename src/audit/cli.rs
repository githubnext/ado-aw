use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

use crate::ado::{
    AdoContext, PATH_SEGMENT, download_build_artifact, get_build, list_build_artifacts,
    resolve_ado_context, resolve_auth,
};
use crate::audit::analyzers::{
    detection, firewall, jobs, mcp, missing, otel, policy, safe_outputs,
};
use crate::audit::cache::{RunSummary, load_run_summary, save_run_summary};
use crate::audit::findings;
use crate::audit::model::{AuditData, ErrorInfo, FileInfo, OverviewData};
use crate::audit::pipeline_graph;
use crate::audit::render;
use crate::audit::url::{ParsedBuildRef, parse_build_ref};

pub struct AuditOptions<'a> {
    pub build_id_or_url: &'a str,
    pub output: &'a Path,
    pub json: bool,
    pub org: Option<&'a str>,
    pub project: Option<&'a str>,
    pub pat: Option<&'a str>,
    pub artifacts: Option<&'a [String]>,
    pub no_cache: bool,
}

pub async fn dispatch(opts: AuditOptions<'_>) -> Result<()> {
    let result = fetch_audit_data_inner(opts).await?;
    render_audit(&result.audit, result.json)?;
    if !result.json && !result.from_cache {
        eprintln!("✓ Audit complete. Reports in {}", result.run_dir.display());
    }
    Ok(())
}

pub async fn fetch_audit_data(opts: AuditOptions<'_>) -> Result<AuditData> {
    Ok(fetch_audit_data_inner(opts).await?.audit)
}

struct FetchAuditDataResult {
    audit: AuditData,
    run_dir: PathBuf,
    json: bool,
    from_cache: bool,
}

async fn fetch_audit_data_inner(opts: AuditOptions<'_>) -> Result<FetchAuditDataResult> {
    let parsed = parse_build_ref(opts.build_id_or_url)?;
    let artifact_filters = normalize_artifact_filters(opts.artifacts)?;
    let cwd = tokio::fs::canonicalize(".")
        .await
        .context("Could not resolve current directory")?;
    let ctx = resolve_audit_context(&cwd, opts.org, opts.project, &parsed).await?;
    let auth = resolve_auth(opts.pat).await?;

    let run_dir = opts.output.join(format!("build-{}", parsed.build_id));
    tokio::fs::create_dir_all(&run_dir)
        .await
        .with_context(|| format!("create audit output directory {}", run_dir.display()))?;

    if !opts.no_cache
        && let Some(summary) = load_run_summary(&run_dir).await?
    {
        if !opts.json {
            eprintln!(
                "Using cached audit from {}",
                summary.processed_at.to_rfc3339()
            );
        }
        let mut audit = summary.audit_data;
        let cached_audit_before_postprocess = audit.clone();
        derive_post_processing(&mut audit, &run_dir).await;
        // Persist recomputed pipeline_graph + findings back to the
        // cached snapshot so subsequent runs see the same canonical
        // AuditData shape; tooling that diffs successive outputs would
        // otherwise observe drift between the saved file and the
        // in-memory result.
        if audit != cached_audit_before_postprocess
            && let Err(error) = save_run_summary(
                &run_dir,
                &RunSummary {
                    ado_aw_version: env!("CARGO_PKG_VERSION").to_string(),
                    build_id: parsed.build_id,
                    processed_at: Utc::now(),
                    audit_data: audit.clone(),
                },
            )
            .await
        {
            warn_and_record(
                &mut audit,
                "audit::cli",
                format!("failed to refresh cached run-summary.json: {error:#}"),
            );
        }
        return Ok(FetchAuditDataResult {
            audit,
            run_dir,
            json: opts.json,
            from_cache: true,
        });
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .context("Failed to create HTTP client")?;

    let build = get_build(&client, &ctx, &auth, parsed.build_id).await?;
    let mut audit = AuditData {
        overview: build_overview(&build, &ctx, parsed.build_id, &run_dir),
        ..AuditData::default()
    };

    let filters = artifact_filters.as_deref();
    let saw_artifact_auth_error = fetch_and_record_artifacts(
        &client,
        &ctx,
        &auth,
        parsed.build_id,
        filters,
        &run_dir,
        &mut audit,
    )
    .await?;

    if saw_artifact_auth_error && !has_any_local_artifacts(&run_dir).await {
        anyhow::bail!(
            "failed to download artifacts and no local cache. Use 'az pipelines runs artifact download --run-id {}' to fetch them manually, then re-run.",
            parsed.build_id
        );
    }

    run_analyzers(
        &client,
        &ctx,
        &auth,
        parsed.build_id,
        filters,
        &run_dir,
        &mut audit,
    )
    .await;
    populate_performance_metrics(&mut audit);

    audit.metrics.error_count = audit.errors.len() as u64;
    derive_post_processing(&mut audit, &run_dir).await;

    save_run_summary(
        &run_dir,
        &RunSummary {
            ado_aw_version: env!("CARGO_PKG_VERSION").to_string(),
            build_id: parsed.build_id,
            processed_at: Utc::now(),
            audit_data: audit.clone(),
        },
    )
    .await?;

    Ok(FetchAuditDataResult {
        audit,
        run_dir,
        json: opts.json,
        from_cache: false,
    })
}

/// Re-run the audit-time enrichment passes that depend on local state
/// (pipeline-graph correlation, metric counters, derived findings).
///
/// Called both after a fresh download and after a cache load so that
/// both code paths produce a structurally identical `AuditData`.
/// `populate_pipeline_graph` failures are downgraded to warnings rather
/// than aborting the audit.
async fn derive_post_processing(audit: &mut AuditData, run_dir: &Path) {
    if let Err(error) = pipeline_graph::populate_pipeline_graph(audit, run_dir).await {
        warn_and_record(
            audit,
            "audit::pipeline_graph",
            format!("pipeline graph correlation failed: {error:#}"),
        );
    }
    audit.metrics.warning_count = audit.warnings.len() as u64;
    findings::derive_findings(audit);
}

/// Download all selected artifacts for the build, recording auth errors and
/// non-fatal download failures as warnings rather than hard failures.
/// Returns `true` if at least one artifact download was blocked by an auth error.
async fn fetch_and_record_artifacts(
    client: &reqwest::Client,
    ctx: &AdoContext,
    auth: &crate::ado::AdoAuth,
    build_id: u64,
    artifact_filters: Option<&[String]>,
    run_dir: &Path,
    audit: &mut AuditData,
) -> Result<bool> {
    let mut saw_artifact_auth_error = false;
    match list_build_artifacts(client, ctx, auth, build_id).await {
        Ok(artifacts) => {
            let selected: Vec<_> = artifacts
                .into_iter()
                .filter(|artifact| artifact_matches_selected(&artifact.name, artifact_filters))
                .collect();

            if selected.is_empty() {
                let message = if artifact_filters.is_some() {
                    "no matching artifacts were published for the selected --artifacts filter"
                        .to_string()
                } else {
                    "no artifacts were published for this build".to_string()
                };
                warn_and_record(audit, "audit::artifacts", message);
            }

            for artifact in selected {
                match download_artifact_preserving_cache(client, auth, &artifact, run_dir).await {
                    Ok(()) => {}
                    Err(error) if is_authz_error(&error) => {
                        saw_artifact_auth_error = true;
                        warn_and_record(
                            audit,
                            "audit::artifacts",
                            format!(
                                "failed to download artifact '{}': {:#}; using any local copy already present",
                                artifact.name, error
                            ),
                        );
                    }
                    Err(error) => {
                        warn_and_record(
                            audit,
                            "audit::artifacts",
                            format!(
                                "failed to download artifact '{}': {:#}",
                                artifact.name, error
                            ),
                        );
                    }
                }
            }
        }
        Err(error) if is_authz_error(&error) => {
            saw_artifact_auth_error = true;
            warn_and_record(
                audit,
                "audit::artifacts",
                format!(
                    "failed to list build artifacts: {:#}; using any local cache already present",
                    error
                ),
            );
        }
        Err(error) => {
            return Err(error).context(format!("failed to list artifacts for build {}", build_id));
        }
    }
    Ok(saw_artifact_auth_error)
}

/// Run all analysis passes over the downloaded artifacts and populate `audit`.
/// Individual analyzer failures are recorded as warnings rather than returned as errors.
async fn run_analyzers(
    client: &reqwest::Client,
    ctx: &AdoContext,
    auth: &crate::ado::AdoAuth,
    build_id: u64,
    artifact_filters: Option<&[String]>,
    run_dir: &Path,
    audit: &mut AuditData,
) {
    match collect_downloaded_files(run_dir, artifact_filters).await {
        Ok(files) => audit.downloaded_files = files,
        Err(error) => warn_and_record(
            audit,
            "audit::artifacts",
            format!("failed to enumerate downloaded files: {:#}", error),
        ),
    }

    if let Some(agent_outputs_dir) = find_artifact_dir(run_dir, "agent_outputs").await {
        run_agent_output_analyzers(&agent_outputs_dir, audit).await;
    }

    match safe_outputs::analyze_safe_outputs(run_dir).await {
        Ok(result) => {
            audit.safe_output_summary = result.summary;
            audit.safe_output_execution = result.execution;
            audit.rejected_safe_outputs = result.rollup;
            audit.created_items = result.created_items;
            audit.key_findings.extend(result.findings);
        }
        Err(error) => warn_and_record(
            audit,
            "audit::safe_outputs",
            format!("safe-output analysis failed: {:#}", error),
        ),
    }

    match detection::analyze_detection(run_dir).await {
        Ok(result) => audit.detection_analysis = result,
        Err(error) => warn_and_record(
            audit,
            "audit::detection",
            format!("detection analysis failed: {:#}", error),
        ),
    }

    match missing::extract_missing_tools(run_dir).await {
        Ok(result) => audit.missing_tools = result,
        Err(error) => warn_and_record(
            audit,
            "audit::missing_tools",
            format!("missing-tool extraction failed: {:#}", error),
        ),
    }
    match missing::extract_missing_data(run_dir).await {
        Ok(result) => audit.missing_data = result,
        Err(error) => warn_and_record(
            audit,
            "audit::missing_data",
            format!("missing-data extraction failed: {:#}", error),
        ),
    }
    match missing::extract_noops(run_dir).await {
        Ok(result) => audit.noops = result,
        Err(error) => warn_and_record(
            audit,
            "audit::noops",
            format!("noop extraction failed: {:#}", error),
        ),
    }

    match jobs::fetch_timeline(client, ctx, auth, build_id).await {
        Ok(timeline) => audit.jobs = jobs::timeline_to_jobs(&timeline),
        Err(error) => warn_and_record(
            audit,
            "audit::jobs",
            format!("job timeline analysis failed: {:#}", error),
        ),
    }
}

/// Run analyzers that operate on the `agent_outputs` artifact directory.
async fn run_agent_output_analyzers(agent_outputs_dir: &Path, audit: &mut AuditData) {
    let firewall_dir = agent_outputs_dir.join("logs").join("firewall");
    match firewall::analyze_firewall_logs(&firewall_dir).await {
        Ok(result) => audit.firewall_analysis = result,
        Err(error) => warn_and_record(
            audit,
            "audit::firewall",
            format!("firewall analysis failed: {:#}", error),
        ),
    }
    match policy::analyze_policy(&firewall_dir).await {
        Ok(result) => audit.policy_analysis = result,
        Err(error) => warn_and_record(
            audit,
            "audit::policy",
            format!("policy analysis failed: {:#}", error),
        ),
    }

    let mcpg_dir = agent_outputs_dir.join("logs").join("mcpg");
    match mcp::analyze_mcp_tool_usage(&mcpg_dir).await {
        Ok(result) => audit.mcp_tool_usage = result,
        Err(error) => warn_and_record(
            audit,
            "audit::mcp",
            format!("MCP tool-usage analysis failed: {:#}", error),
        ),
    }
    match mcp::analyze_mcp_server_health(&mcpg_dir).await {
        Ok(result) => audit.mcp_server_health = result,
        Err(error) => warn_and_record(
            audit,
            "audit::mcp",
            format!("MCP server-health analysis failed: {:#}", error),
        ),
    }
    match mcp::extract_mcp_failures(&mcpg_dir).await {
        Ok(result) => audit.mcp_failures = result,
        Err(error) => warn_and_record(
            audit,
            "audit::mcp",
            format!("MCP failure extraction failed: {:#}", error),
        ),
    }

    match otel::analyze_otel(agent_outputs_dir).await {
        Ok(result) => {
            audit.metrics = result.metrics;
            audit.engine_config = result.engine_config;
            audit.performance_metrics = result.performance;
            audit.overview.aw_info = result.aw_info;
        }
        Err(error) => warn_and_record(
            audit,
            "audit::otel",
            format!("OTel analysis failed: {:#}", error),
        ),
    }
}

/// Backfill performance metric fields that can be derived from other already-populated
/// analysis results (firewall request count, most-used MCP tool).
fn populate_performance_metrics(audit: &mut AuditData) {
    if let Some(firewall_analysis) = &audit.firewall_analysis {
        let performance = audit.performance_metrics.get_or_insert_default();
        if performance.network_requests.is_none() {
            performance.network_requests = Some(firewall_analysis.total_requests);
        }
    }
    if let Some(mcp_tool_usage) = &audit.mcp_tool_usage
        && let Some(tool) = mcp_tool_usage.tools.first()
    {
        let performance = audit.performance_metrics.get_or_insert_default();
        if performance.most_used_tool.is_none() && !tool.name.is_empty() {
            performance.most_used_tool = Some(tool.name.clone());
        }
    }
}

async fn resolve_audit_context(
    cwd: &Path,
    org: Option<&str>,
    project: Option<&str>,
    parsed: &ParsedBuildRef,
) -> Result<AdoContext> {
    // First, validate the URL host (if any) against a trust anchor derived
    // independently of full context resolution. We deliberately do NOT depend
    // on `resolve_ado_context` here because that helper requires both --org
    // AND --project when running outside a git repo — but when the user
    // supplies a full build URL, the project is already in the URL and asking
    // them to also pass --project would be a UX regression.
    if let Some(url_host) = parsed.host.as_deref() {
        let trusted_host = resolve_trusted_host(cwd, org).await;
        validate_audit_url_host(url_host, trusted_host.as_deref())?;
    }

    // Best-effort full context (git remote + --org + --project). Used only as
    // a source of defaults that the URL overrides supersede.
    let trusted_ctx = resolve_ado_context(cwd, org, project).await.ok();

    if parsed.org.is_some() && parsed.project.is_some() && parsed.host.is_some() {
        let mut ctx = trusted_ctx.unwrap_or_else(|| AdoContext {
            org_url: String::new(),
            project: String::new(),
            repo_name: String::new(),
        });
        apply_parsed_context_overrides(&mut ctx, parsed);
        return Ok(ctx);
    }

    trusted_ctx.ok_or_else(|| {
        anyhow::anyhow!(
            "Could not determine ADO context: pass a full build URL, or pass --org and --project, or run from inside an ADO git repository."
        )
    })
}

/// Resolve the host we are willing to authenticate to, *without* requiring
/// a full ADO context (which would also need --project to be present).
///
/// Trust anchor priority:
/// 1. `--org` (any form `resolve_ado_context` accepts), if it normalizes to
///    a parseable URL.
/// 2. The git remote of `cwd`, if it parses as an ADO remote.
/// 3. None — the caller must then refuse any non-cloud URL host.
async fn resolve_trusted_host(cwd: &Path, org_flag: Option<&str>) -> Option<String> {
    if let Some(org) = org_flag {
        let normalized = crate::ado::normalize_org_url(org);
        if let Some(host) = host_from_org_url(&normalized) {
            return Some(host);
        }
    }

    let remote = crate::ado::get_git_remote_url(cwd).await.ok()?;
    let remote_ctx = crate::ado::parse_ado_remote(&remote).ok()?;
    host_from_org_url(&remote_ctx.org_url)
}

/// Refuse to authenticate against an arbitrary host derived from a
/// user-supplied build URL.
///
/// Without this check, a user social-engineered into running
/// `ado-aw audit https://attacker.example.com/Collection/Project/_build/results?buildId=1`
/// would silently send their ADO PAT (from `--pat` /
/// `AZURE_DEVOPS_EXT_PAT` / az CLI fallback) to the attacker over HTTP
/// Basic Auth.
///
/// Trust rules:
/// - Microsoft-managed cloud hosts (`dev.azure.com`, `*.visualstudio.com`)
///   are always allowed.
/// - Any other host (typically on-prem ADO Server) is allowed only when it
///   matches `trusted_host`, which the caller derives from either `--org`
///   or the local git remote — both explicit, locally controlled trust
///   anchors.
fn validate_audit_url_host(url_host: &str, trusted_host: Option<&str>) -> Result<()> {
    if is_microsoft_cloud_host(url_host) {
        return Ok(());
    }

    match trusted_host {
        Some(expected) if expected.eq_ignore_ascii_case(url_host) => Ok(()),
        Some(expected) => anyhow::bail!(
            "Refusing to send ADO credentials to host '{}': it does not match \
             the expected ADO host '{}' (resolved from --org / git remote). \
             If this on-prem host is intentional, pass --org pointing at it \
             (e.g. --org https://{}/<collection>) to authorize.",
            url_host,
            expected,
            url_host
        ),
        None => anyhow::bail!(
            "Refusing to send ADO credentials to unrecognized host '{}'. \
             Only Microsoft-managed cloud hosts (dev.azure.com, *.visualstudio.com) \
             are trusted by default. For an on-prem host, pass \
             --org https://{}/<collection> to confirm this host is trusted.",
            url_host,
            url_host
        ),
    }
}

fn is_microsoft_cloud_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    if host == "dev.azure.com" {
        return true;
    }
    // Require `<non-empty-label>.visualstudio.com`. `strip_suffix` on a
    // bare-suffix string like ".visualstudio.com" yields an empty prefix,
    // which we explicitly reject.
    match host.strip_suffix(".visualstudio.com") {
        Some(prefix) => !prefix.is_empty(),
        None => false,
    }
}

fn host_from_org_url(org_url: &str) -> Option<String> {
    if org_url.is_empty() {
        return None;
    }
    url::Url::parse(org_url)
        .ok()?
        .host_str()
        .map(str::to_string)
}

fn apply_parsed_context_overrides(ctx: &mut AdoContext, parsed: &ParsedBuildRef) {
    if let Some(org_url) = parsed_org_url(parsed) {
        ctx.org_url = org_url;
    }
    if let Some(project) = &parsed.project {
        ctx.project = project.clone();
    }
}

fn parsed_org_url(parsed: &ParsedBuildRef) -> Option<String> {
    let org = parsed.org.as_deref()?;
    let host = parsed.host.as_deref()?;

    if host.eq_ignore_ascii_case("dev.azure.com") {
        Some(format!("https://{host}/{org}"))
    } else if host.to_ascii_lowercase().ends_with(".visualstudio.com") {
        Some(format!("https://{host}"))
    } else {
        Some(format!("https://{host}/{org}"))
    }
}

fn build_overview(
    build: &serde_json::Value,
    ctx: &AdoContext,
    build_id: u64,
    run_dir: &Path,
) -> OverviewData {
    let started_at = string_field(build, &["startTime"]);
    let finished_at = string_field(build, &["finishTime"]);

    OverviewData {
        build_id: build
            .get("id")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(build_id),
        pipeline_name: build
            .get("definition")
            .and_then(|value| value.get("name"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        status: build
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        result: string_field(build, &["result"]),
        created_at: string_field(build, &["queueTime", "createdDate", "creationTime"]),
        started_at: started_at.clone(),
        finished_at: finished_at.clone(),
        duration: format_duration(started_at.as_deref(), finished_at.as_deref()),
        source_branch: string_field(build, &["sourceBranch"]),
        source_version: string_field(build, &["sourceVersion"]),
        url: Some(build_audit_url(ctx, build_id)),
        logs_path: Some(run_dir.display().to_string()),
        aw_info: None,
    }
}

fn string_field(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .filter_map(|key| value.get(*key).and_then(serde_json::Value::as_str))
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn build_audit_url(ctx: &AdoContext, build_id: u64) -> String {
    format!(
        "{}/{}/_build/results?buildId={}",
        ctx.org_url.trim_end_matches('/'),
        percent_encoding::utf8_percent_encode(&ctx.project, PATH_SEGMENT),
        build_id
    )
}

fn format_duration(started_at: Option<&str>, finished_at: Option<&str>) -> Option<String> {
    let start = DateTime::parse_from_rfc3339(started_at?).ok()?;
    let finish = DateTime::parse_from_rfc3339(finished_at?).ok()?;
    let delta = finish.signed_duration_since(start);
    if delta.num_seconds() < 0 {
        return None;
    }
    Some(format!(
        "{}m {}s",
        delta.num_seconds() / 60,
        delta.num_seconds() % 60
    ))
}

fn normalize_artifact_filters(filters: Option<&[String]>) -> Result<Option<Vec<String>>> {
    let Some(filters) = filters else {
        return Ok(None);
    };

    let mut normalized = Vec::new();
    for filter in filters {
        let filter = filter.trim().to_ascii_lowercase();
        let canonical = match filter.as_str() {
            "agent" => "agent",
            "detection" => "detection",
            "safe-outputs" | "safe_outputs" => "safe-outputs",
            _ => anyhow::bail!(
                "Invalid --artifacts value '{}'. Valid values: agent, detection, safe-outputs.",
                filter
            ),
        };
        if !normalized.iter().any(|existing| existing == canonical) {
            normalized.push(canonical.to_string());
        }
    }

    Ok(Some(normalized))
}

fn artifact_matches_selected(name: &str, filters: Option<&[String]>) -> bool {
    let Some(filters) = filters else {
        return artifact_name_to_prefix(name).is_some();
    };
    let Some(prefix) = artifact_name_to_prefix(name) else {
        return false;
    };
    filters.iter().any(|filter| match filter.as_str() {
        "agent" => prefix == "agent_outputs",
        "detection" => prefix == "analyzed_outputs",
        "safe-outputs" => prefix == "safe_outputs",
        _ => false,
    })
}

fn artifact_name_to_prefix(name: &str) -> Option<&'static str> {
    if name == "agent_outputs" || name.starts_with("agent_outputs_") {
        Some("agent_outputs")
    } else if name == "analyzed_outputs" || name.starts_with("analyzed_outputs_") {
        Some("analyzed_outputs")
    } else if name == "safe_outputs" || name.starts_with("safe_outputs_") {
        Some("safe_outputs")
    } else {
        None
    }
}

async fn download_artifact_preserving_cache(
    client: &reqwest::Client,
    auth: &crate::ado::AdoAuth,
    artifact: &crate::ado::BuildArtifact,
    run_dir: &Path,
) -> Result<()> {
    let artifact_dir = run_dir.join(&artifact.name);
    let backup_dir = run_dir.join(format!("{}.cached", artifact.name));
    let had_existing = tokio::fs::metadata(&artifact_dir).await.is_ok();

    if tokio::fs::metadata(&backup_dir).await.is_ok() {
        let _ = tokio::fs::remove_dir_all(&backup_dir).await;
    }
    if had_existing {
        tokio::fs::rename(&artifact_dir, &backup_dir)
            .await
            .with_context(|| {
                format!(
                    "backup existing artifact directory {} before redownload",
                    artifact_dir.display()
                )
            })?;
    }

    match download_build_artifact(client, auth, artifact, run_dir).await {
        Ok(()) => {
            if had_existing {
                let _ = tokio::fs::remove_dir_all(&backup_dir).await;
            }
            Ok(())
        }
        Err(error) => {
            if tokio::fs::metadata(&artifact_dir).await.is_ok() {
                let _ = tokio::fs::remove_dir_all(&artifact_dir).await;
            }
            if had_existing {
                tokio::fs::rename(&backup_dir, &artifact_dir)
                    .await
                    .with_context(|| {
                        format!(
                            "restore cached artifact directory {} after failed download",
                            artifact_dir.display()
                        )
                    })?;
            }
            Err(error)
        }
    }
}

async fn has_any_local_artifacts(run_dir: &Path) -> bool {
    for prefix in ["agent_outputs", "analyzed_outputs", "safe_outputs"] {
        if find_artifact_dir(run_dir, prefix).await.is_some() {
            return true;
        }
    }
    false
}

async fn collect_downloaded_files(
    run_dir: &Path,
    filters: Option<&[String]>,
) -> Result<Vec<FileInfo>> {
    let mut files = Vec::new();
    for prefix in selected_prefixes(filters) {
        if let Some(artifact_dir) = find_artifact_dir(run_dir, prefix).await {
            files.extend(collect_files_under(run_dir, &artifact_dir).await?);
        }
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

fn selected_prefixes(filters: Option<&[String]>) -> Vec<&'static str> {
    match filters {
        Some(filters) => {
            let mut prefixes = Vec::new();
            for filter in filters {
                let prefix = match filter.as_str() {
                    "agent" => "agent_outputs",
                    "detection" => "analyzed_outputs",
                    "safe-outputs" => "safe_outputs",
                    _ => continue,
                };
                if !prefixes.contains(&prefix) {
                    prefixes.push(prefix);
                }
            }
            prefixes
        }
        None => vec!["agent_outputs", "analyzed_outputs", "safe_outputs"],
    }
}

async fn collect_files_under(run_dir: &Path, start_dir: &Path) -> Result<Vec<FileInfo>> {
    let mut files = Vec::new();
    let mut stack = vec![start_dir.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let mut entries = tokio::fs::read_dir(&dir)
            .await
            .with_context(|| format!("read artifact directory {}", dir.display()))?;
        while let Some(entry) = entries
            .next_entry()
            .await
            .with_context(|| format!("iterate artifact directory {}", dir.display()))?
        {
            let path = entry.path();
            let file_type = entry
                .file_type()
                .await
                .with_context(|| format!("inspect artifact path {}", path.display()))?;
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }

            let metadata = entry
                .metadata()
                .await
                .with_context(|| format!("stat artifact file {}", path.display()))?;
            let relative = path
                .strip_prefix(run_dir)
                .unwrap_or(path.as_path())
                .display()
                .to_string();
            files.push(FileInfo {
                path: relative,
                size_bytes: metadata.len(),
                sha256: None,
            });
        }
    }

    Ok(files)
}

async fn find_artifact_dir(run_dir: &Path, prefix: &str) -> Option<PathBuf> {
    let mut entries = tokio::fs::read_dir(run_dir).await.ok()?;
    let mut hits: Vec<(String, PathBuf)> = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false)
            && let Some(name) = entry.file_name().to_str()
            && (name == prefix || name.starts_with(&format!("{}_", prefix)))
        {
            hits.push((name.to_string(), entry.path()));
        }
    }
    // Numeric-suffix sort so `agent_outputs_10` outranks
    // `agent_outputs_9` (lexicographic sort gets this wrong).
    hits.sort_by(|(a, _), (b, _)| crate::audit::cmp_numeric_suffix(a, b));
    hits.pop().map(|(_, path)| path)
}

fn is_authz_error(error: &anyhow::Error) -> bool {
    let message = error.to_string().to_ascii_lowercase();
    message.contains("ado api returned 401") || message.contains("ado api returned 403")
}

fn warn_and_record(audit: &mut AuditData, source: &str, message: String) {
    eprintln!("warning: {message}");
    audit.warnings.push(ErrorInfo {
        source: source.to_string(),
        message,
        timestamp: None,
    });
}

fn render_audit(audit: &AuditData, json: bool) -> Result<()> {
    if json {
        let mut stdout = io::stdout().lock();
        render::json::render_json(audit, &mut stdout)?;
    } else {
        print!("{}", render::console::render_console(audit));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_context_overrides_flag_org() {
        let parsed =
            parse_build_ref("https://dev.azure.com/url-org/My%20Project/_build/results?buildId=42")
                .expect("parse build ref");
        let mut ctx = AdoContext {
            org_url: String::from("https://dev.azure.com/flag-org"),
            project: String::from("FlagProject"),
            repo_name: String::from("repo"),
        };

        apply_parsed_context_overrides(&mut ctx, &parsed);

        assert_eq!(ctx.org_url, "https://dev.azure.com/url-org");
        assert_eq!(ctx.project, "My Project");
    }

    #[tokio::test]
    async fn find_artifact_dir_picks_highest_numbered_match() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        tokio::fs::create_dir_all(temp_dir.path().join("agent_outputs_001"))
            .await
            .expect("create first dir");
        tokio::fs::create_dir_all(temp_dir.path().join("agent_outputs_999"))
            .await
            .expect("create second dir");
        tokio::fs::create_dir_all(temp_dir.path().join("safe_outputs"))
            .await
            .expect("create safe outputs dir");

        let found = find_artifact_dir(temp_dir.path(), "agent_outputs")
            .await
            .expect("find artifact dir");

        assert_eq!(
            found.file_name().and_then(|name| name.to_str()),
            Some("agent_outputs_999")
        );
    }

    /// Regression test: lexicographic sort would pick `agent_outputs_9`
    /// here (because `'9' > '1'`); numeric-suffix sort must pick
    /// `agent_outputs_10` instead.
    #[tokio::test]
    async fn find_artifact_dir_orders_multi_digit_suffixes_numerically() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        for suffix in ["1", "2", "9", "10", "100"] {
            tokio::fs::create_dir_all(temp_dir.path().join(format!("agent_outputs_{suffix}")))
                .await
                .expect("create dir");
        }

        let found = find_artifact_dir(temp_dir.path(), "agent_outputs")
            .await
            .expect("find artifact dir");

        assert_eq!(
            found.file_name().and_then(|name| name.to_str()),
            Some("agent_outputs_100")
        );
    }

    #[test]
    fn artifact_filter_mapping_matches_expected_sets() {
        let filters = vec![
            String::from("agent"),
            String::from("detection"),
            String::from("safe-outputs"),
        ];
        let normalized = normalize_artifact_filters(Some(&filters)).expect("normalize filters");
        let normalized = normalized.as_deref();

        assert!(artifact_matches_selected("agent_outputs_42", normalized));
        assert!(artifact_matches_selected("analyzed_outputs_42", normalized));
        assert!(artifact_matches_selected("safe_outputs", normalized));

        let agent_only = vec![String::from("agent")];
        let agent_only =
            normalize_artifact_filters(Some(&agent_only)).expect("normalize agent filter");
        let agent_only = agent_only.as_deref();
        assert!(artifact_matches_selected("agent_outputs_42", agent_only));
        assert!(!artifact_matches_selected(
            "analyzed_outputs_42",
            agent_only
        ));
        assert!(!artifact_matches_selected("safe_outputs", agent_only));
    }

    // ── validate_audit_url_host: PAT exfiltration guard ───────────────────

    #[test]
    fn validate_host_accepts_dev_azure_com_without_trusted_context() {
        validate_audit_url_host("dev.azure.com", None).expect("cloud host is always trusted");
    }

    #[test]
    fn validate_host_accepts_dev_azure_com_case_insensitively() {
        validate_audit_url_host("Dev.Azure.Com", None)
            .expect("cloud host match is case-insensitive");
    }

    #[test]
    fn validate_host_accepts_visualstudio_com_subdomain() {
        validate_audit_url_host("myorg.visualstudio.com", None)
            .expect("legacy visualstudio.com host is trusted");
    }

    #[test]
    fn validate_host_rejects_lookalike_attacker_visualstudio_com_path() {
        let attacker = "visualstudio.com.attacker.example";
        let err = validate_audit_url_host(attacker, None)
            .expect_err("attacker-controlled host with visualstudio.com prefix must be rejected");
        assert!(
            err.to_string().contains("Refusing to send"),
            "expected refusal message, got: {err}"
        );
    }

    #[test]
    fn validate_host_rejects_arbitrary_host_without_trusted_context() {
        let err = validate_audit_url_host("attacker.example.com", None)
            .expect_err("on-prem host with no trusted context must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("attacker.example.com"),
            "error should name the rejected host, got: {msg}"
        );
        assert!(
            msg.contains("--org"),
            "error should suggest --org as the remediation, got: {msg}"
        );
    }

    #[test]
    fn validate_host_rejects_host_mismatch_against_trusted_context() {
        let err = validate_audit_url_host("attacker.example.com", Some("onprem-real.example.com"))
            .expect_err("URL host that doesn't match --org / git remote must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("attacker.example.com"),
            "error should name the rejected host, got: {msg}"
        );
        assert!(
            msg.contains("onprem-real.example.com"),
            "error should name the expected host, got: {msg}"
        );
    }

    #[test]
    fn validate_host_accepts_matching_onprem_host() {
        validate_audit_url_host("onprem.example.com", Some("onprem.example.com"))
            .expect("on-prem host matching --org / git remote must be accepted");
    }

    #[test]
    fn validate_host_accepts_matching_onprem_host_case_insensitively() {
        validate_audit_url_host("onprem.example.com", Some("OnPrem.Example.Com"))
            .expect("on-prem host match must be case-insensitive");
    }

    #[test]
    fn validate_host_rejects_unrelated_host_even_with_cloud_trusted_context() {
        // A user with a dev.azure.com trusted context is still protected
        // against URLs pointing at an unrelated on-prem host.
        let err = validate_audit_url_host("attacker.example.com", Some("dev.azure.com"))
            .expect_err("must reject attacker host even when trusted context is cloud");
        assert!(
            err.to_string().contains("attacker.example.com"),
            "error should name the rejected host"
        );
    }

    #[test]
    fn is_microsoft_cloud_host_classifies_correctly() {
        assert!(is_microsoft_cloud_host("dev.azure.com"));
        assert!(is_microsoft_cloud_host("DEV.AZURE.COM"));
        assert!(is_microsoft_cloud_host("myorg.visualstudio.com"));
        assert!(is_microsoft_cloud_host("MyOrg.VisualStudio.Com"));

        assert!(!is_microsoft_cloud_host(""));
        assert!(!is_microsoft_cloud_host("dev.azure.com.attacker.example"));
        assert!(!is_microsoft_cloud_host("notdev.azure.com"));
        assert!(!is_microsoft_cloud_host("visualstudio.com"));
        assert!(!is_microsoft_cloud_host(".visualstudio.com"));
        assert!(!is_microsoft_cloud_host("xvisualstudio.com"));
    }

    #[test]
    fn host_from_org_url_extracts_and_handles_empty() {
        assert_eq!(
            host_from_org_url("https://dev.azure.com/my-org").as_deref(),
            Some("dev.azure.com")
        );
        assert_eq!(
            host_from_org_url("https://onprem.example.com/Coll").as_deref(),
            Some("onprem.example.com")
        );
        assert_eq!(host_from_org_url(""), None);
        assert_eq!(host_from_org_url("not a url"), None);
    }

    // ── resolve_trusted_host: trust anchor without requiring --project ────

    #[tokio::test]
    async fn resolve_trusted_host_uses_org_flag_full_url() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let host = resolve_trusted_host(
            temp_dir.path(),
            Some("https://onprem.example.com/MyCollection"),
        )
        .await;
        assert_eq!(host.as_deref(), Some("onprem.example.com"));
    }

    #[tokio::test]
    async fn resolve_trusted_host_uses_org_flag_bare_name() {
        // Bare org name is normalized to https://dev.azure.com/<org>.
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let host = resolve_trusted_host(temp_dir.path(), Some("my-org")).await;
        assert_eq!(host.as_deref(), Some("dev.azure.com"));
    }

    #[tokio::test]
    async fn resolve_trusted_host_returns_none_in_arbitrary_folder_without_org_flag() {
        // No git remote, no --org → no trust anchor. The caller must then
        // refuse any non-cloud URL host.
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let host = resolve_trusted_host(temp_dir.path(), None).await;
        assert_eq!(host, None);
    }

    #[tokio::test]
    async fn resolve_trusted_host_with_only_org_flag_enables_onprem_url() {
        // Regression: running from an arbitrary folder with --org pointing at
        // an on-prem host (and NO --project, because project is in the URL)
        // must yield a trust anchor and let validation pass.
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let host = resolve_trusted_host(temp_dir.path(), Some("https://onprem.example.com/Coll"))
            .await
            .expect("--org alone should establish a trusted host");
        validate_audit_url_host("onprem.example.com", Some(&host))
            .expect("URL host matching --org must pass validation");
    }
}
