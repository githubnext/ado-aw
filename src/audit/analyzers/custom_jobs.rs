//! Job-level audit correlation for custom safe-output jobs.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::Context;
use log::warn;
use serde::Deserialize;

use crate::audit::model::{
    AuditData, AwInfo, ComponentProvenance, CustomSafeOutputAdoJob, CustomSafeOutputJobAudit,
    CustomSafeOutputJobMetadata, ErrorInfo, Finding, JobData, Severity,
};
use crate::compile::ir::summary::{JobSummary, PipelineSummary};
use crate::ndjson::{SAFE_OUTPUT_FILENAME, read_ndjson_file};

const CUSTOM_TOOLS_FILENAME: &str = "custom-tools.json";

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct CustomToolConfig {
    name: String,
    #[serde(rename = "inputSchema")]
    input_schema: Option<serde_json::Value>,
    output: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ResolvedCustomToolConfig {
    custom_tools: Vec<CustomToolConfig>,
    tool_configs: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default)]
struct CatalogEntry {
    proposal_time_acknowledgement: Option<String>,
    metadata: Option<CustomSafeOutputJobMetadata>,
    component: Option<ComponentProvenance>,
    config_schema_digest: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct CustomToolCatalog {
    entries: BTreeMap<String, CatalogEntry>,
    pub(crate) warnings: Vec<ErrorInfo>,
}

impl CustomToolCatalog {
    pub(crate) fn names(&self) -> BTreeSet<String> {
        self.entries.keys().cloned().collect()
    }
}

/// Load every custom tool name currently available to audit.
///
/// The compiler-generated `custom-tools.json` is the primary source. The
/// compile-time `aw_info.json` metadata is merged so older cached artifacts
/// that lack the tool config can still exclude imported custom tools from the
/// built-in per-item execution audit.
pub(crate) async fn load_custom_tool_catalog(
    download_root: &Path,
    supplied_aw_info: Option<&AwInfo>,
) -> anyhow::Result<CustomToolCatalog> {
    let mut catalog = CustomToolCatalog::default();
    let mut primary_catalog_loaded = false;

    if let Some(path) = find_metadata_file(download_root, CUSTOM_TOOLS_FILENAME).await? {
        let contents = tokio::fs::read_to_string(&path)
            .await
            .with_context(|| format!("Failed to read custom tool config {}", path.display()))?;
        let value: serde_json::Value = serde_json::from_str(&contents)
            .with_context(|| format!("Failed to parse custom tool config {}", path.display()))?;
        let resolved: ResolvedCustomToolConfig = if value.is_array() {
            ResolvedCustomToolConfig {
                custom_tools: serde_json::from_value(value).with_context(|| {
                    format!("Failed to parse custom tool definitions {}", path.display())
                })?,
                ..Default::default()
            }
        } else {
            serde_json::from_value(value).with_context(|| {
                format!(
                    "Failed to parse resolved custom tool config {}",
                    path.display()
                )
            })?
        };
        primary_catalog_loaded = true;
        for config in resolved.custom_tools {
            let tool = config.name.trim();
            if tool.is_empty() {
                continue;
            }
            let entry = catalog.entries.entry(tool.to_string()).or_default();
            entry.proposal_time_acknowledgement = normalize_optional_string(config.output);
            let staged = resolved
                .tool_configs
                .get(tool)
                .and_then(|value| value.get("staged"))
                .and_then(serde_json::Value::as_bool);
            if let Some(staged) = staged {
                entry
                    .metadata
                    .get_or_insert_with(CustomSafeOutputJobMetadata::default)
                    .staged_requested = Some(staged);
            }
            entry.config_schema_digest = config.input_schema.as_ref().and_then(|schema| {
                serde_json::to_vec(schema)
                    .ok()
                    .map(|bytes| crate::hash::sha256_hex(&bytes))
            });
        }
    }

    let disk_aw_info = if supplied_aw_info.is_none() {
        match load_aw_info(download_root).await {
            Ok(value) => value,
            Err(error) if primary_catalog_loaded => {
                warn!("Failed to read optional aw_info.json metadata: {error:#}");
                catalog
                    .warnings
                    .push(crate::audit::malformed_aw_info_warning());
                None
            }
            Err(error) => return Err(error),
        }
    } else {
        None
    };
    if let Some(aw_info) = supplied_aw_info.or(disk_aw_info.as_ref()) {
        merge_aw_info(&mut catalog, aw_info);
    }

    Ok(catalog)
}

fn merge_aw_info(catalog: &mut CustomToolCatalog, aw_info: &AwInfo) {
    for component in &aw_info.custom_components {
        let tool = component.tool.trim();
        if tool.is_empty() {
            continue;
        }
        let entry = catalog.entries.entry(tool.to_string()).or_default();
        entry.component.get_or_insert_with(|| component.clone());
    }

    for metadata in &aw_info.custom_jobs {
        let tool = metadata.tool.trim();
        if tool.is_empty() {
            continue;
        }
        let entry = catalog.entries.entry(tool.to_string()).or_default();
        let merged = entry
            .metadata
            .get_or_insert_with(CustomSafeOutputJobMetadata::default);
        if merged.tool.is_empty() {
            merged.tool = metadata.tool.clone();
        }
        if merged.job_id.is_none() {
            merged.job_id = metadata.job_id.clone();
        }
        if merged.approval_path.is_none() {
            merged.approval_path = metadata.approval_path.clone();
        }
        if merged.staged_requested.is_none() {
            merged.staged_requested = metadata.staged_requested;
        }
    }
}

/// Populate `AuditData.custom_safe_output_jobs` from proposal artifacts,
/// compile-time metadata, the typed graph, and the ADO timeline.
pub async fn populate_custom_safe_output_jobs(
    audit: &mut AuditData,
    download_root: &Path,
) -> anyhow::Result<()> {
    let previous = audit.custom_safe_output_jobs.clone();
    let mut catalog =
        load_custom_tool_catalog(download_root, audit.overview.aw_info.as_ref()).await?;
    for warning in &catalog.warnings {
        crate::audit::push_warning_once(audit, warning.clone());
    }
    let proposal_counts = load_proposal_counts(download_root).await?;

    discover_custom_proposal_tools(audit, &proposal_counts, &mut catalog);
    for previous_entry in &previous {
        catalog
            .entries
            .entry(previous_entry.tool.clone())
            .or_default();
    }

    if catalog.entries.is_empty() {
        return Ok(());
    }

    let mut findings = Vec::new();
    let mut reports = Vec::with_capacity(catalog.entries.len());

    for (tool, entry) in catalog.entries {
        let graph_job_id = graph_job_id_for_tool(audit, &tool);
        let graph_display_name = graph_job_id
            .as_deref()
            .and_then(|job_id| unique_graph_display_name(audit, job_id));
        let metadata_job_id = entry
            .metadata
            .as_ref()
            .and_then(|metadata| normalize_optional_string(metadata.job_id.clone()));
        if let (Some(graph_job_id), Some(metadata_job_id)) =
            (graph_job_id.as_deref(), metadata_job_id.as_deref())
            && graph_job_id != metadata_job_id
        {
            findings.push(Finding {
                category: String::from("safe_outputs"),
                severity: Severity::High,
                title: format!("Custom job metadata identity mismatch for {tool}"),
                description: format!(
                    "The typed pipeline graph assigns custom tool '{tool}' to ADO job \
                     '{graph_job_id}', but the aw_info marker claims '{metadata_job_id}'."
                ),
                impact: Some(String::from(
                    "Untrusted runtime metadata does not match the compiler-derived job identity.",
                )),
            });
        }
        let expected_job_id = graph_job_id.clone().or(metadata_job_id);

        let metadata_approval_path = entry
            .metadata
            .as_ref()
            .and_then(|metadata| normalize_optional_string(metadata.approval_path.clone()));
        let graph_approval_path = graph_job_id
            .as_deref()
            .and_then(|job_id| approval_path_from_graph(audit, job_id));
        if graph_job_id.is_some()
            && metadata_approval_path.is_some()
            && metadata_approval_path != graph_approval_path
        {
            findings.push(Finding {
                category: String::from("safe_outputs"),
                severity: Severity::High,
                title: format!("Custom job metadata approval mismatch for {tool}"),
                description: format!(
                    "The typed pipeline graph assigns custom tool '{tool}' to approval path \
                     '{}', but the aw_info marker claims '{}'.",
                    graph_approval_path.as_deref().unwrap_or("automatic"),
                    metadata_approval_path.as_deref().unwrap_or("automatic")
                ),
                impact: Some(String::from(
                    "Untrusted runtime metadata does not match the compiler-derived approval path.",
                )),
            });
        }
        let graph_is_authoritative = graph_job_id.is_some();
        let approval_path = if graph_is_authoritative {
            graph_approval_path
        } else {
            metadata_approval_path
        };
        if let (Some(component), Some(config_digest)) = (
            entry.component.as_ref(),
            entry.config_schema_digest.as_ref(),
        ) && !component.schema_digest.is_empty()
            && component.schema_digest != *config_digest
        {
            findings.push(Finding {
                category: String::from("safe_outputs"),
                severity: Severity::High,
                title: format!("Custom component provenance mismatch for {tool}"),
                description: format!(
                    "The aw_info marker declares schema_digest={} for custom tool '{tool}', but compiler-generated custom-tools.json hashes to {}.",
                    component.schema_digest, config_digest
                ),
                impact: Some(String::from(
                    "The proposal schema does not match the compile-time provenance recorded for the custom component.",
                )),
            });
        }

        let matches = matching_timeline_jobs(
            &audit.jobs,
            &tool,
            expected_job_id.as_deref(),
            graph_display_name.as_deref(),
        );
        let selected = matches.last().copied();

        let previous_entry = previous.iter().find(|candidate| candidate.tool == tool);
        let ado_job = selected
            .map(custom_ado_job_from_timeline)
            .or_else(|| previous_entry.and_then(|entry| entry.ado_job.clone()));
        let expected_job_id = expected_job_id
            .or_else(|| previous_entry.and_then(|entry| entry.expected_job_id.clone()));
        let staged_requested = entry
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.staged_requested)
            .or_else(|| previous_entry.and_then(|entry| entry.staged_requested));
        let component_provenance = entry
            .component
            .or_else(|| previous_entry.and_then(|entry| entry.component_provenance.clone()));
        let proposal_time_acknowledgement = entry.proposal_time_acknowledgement.or_else(|| {
            previous_entry.and_then(|entry| entry.proposal_time_acknowledgement.clone())
        });

        reports.push(CustomSafeOutputJobAudit {
            tool: tool.clone(),
            proposed_count: proposal_counts.get(&tool).copied().unwrap_or(0),
            expected_job_id,
            component_provenance,
            approval_path: if graph_is_authoritative {
                approval_path
            } else {
                approval_path
                    .or_else(|| previous_entry.and_then(|entry| entry.approval_path.clone()))
            },
            staged_requested,
            proposal_time_acknowledgement,
            ado_job,
        });
    }

    add_report_findings(audit, &reports, &mut findings);

    audit.custom_safe_output_jobs = reports;
    for finding in findings {
        if !audit.key_findings.contains(&finding) {
            audit.key_findings.push(finding);
        }
    }
    Ok(())
}

fn discover_custom_proposal_tools(
    audit: &AuditData,
    proposal_counts: &BTreeMap<String, u64>,
    catalog: &mut CustomToolCatalog,
) {
    for tool in proposal_counts.keys() {
        if catalog.entries.contains_key(tool) {
            continue;
        }
        let graph_match = graph_job_id_for_tool(audit, tool).is_some();
        let timeline_match = audit
            .jobs
            .iter()
            .any(|job| job_matches_tool(job, tool, None));
        if graph_match || timeline_match {
            catalog
                .entries
                .insert(tool.clone(), CatalogEntry::default());
        }
    }
}

fn add_report_findings(
    audit: &AuditData,
    reports: &[CustomSafeOutputJobAudit],
    findings: &mut Vec<Finding>,
) {
    for report in reports {
        if let Some(component) = &report.component_provenance {
            let missing = [
                ("source", component.source.trim().is_empty()),
                ("sha", component.sha.trim().is_empty()),
                (
                    "manifest_digest",
                    component.manifest_digest.trim().is_empty(),
                ),
                ("schema_digest", component.schema_digest.trim().is_empty()),
            ]
            .into_iter()
            .filter_map(|(name, missing)| missing.then_some(name))
            .collect::<Vec<_>>();
            if !missing.is_empty() {
                findings.push(Finding {
                    category: String::from("safe_outputs"),
                    severity: Severity::High,
                    title: format!(
                        "Incomplete custom component provenance for {}",
                        report.tool
                    ),
                    description: format!(
                        "The compile-time aw_info marker is missing {} for custom tool '{}'.",
                        missing.join(", "),
                        report.tool
                    ),
                    impact: Some(String::from(
                        "The exact custom component revision or schema cannot be independently identified.",
                    )),
                });
            }
        }

        if let (Some(expected), Some(actual)) =
            (report.expected_job_id.as_deref(), report.ado_job.as_ref())
            && stable_ado_job_identity_matches(actual, expected) == Some(false)
        {
            findings.push(Finding {
                category: String::from("safe_outputs"),
                severity: Severity::High,
                title: format!("Custom job identity mismatch for {}", report.tool),
                description: format!(
                    "Custom tool '{}' was compiled for ADO job '{}', but the correlated timeline job identifies as '{}'.",
                    report.tool,
                    expected,
                    best_ado_job_identity(actual)
                ),
                impact: Some(String::from(
                    "The observed job may not be the compiler-approved executor for these proposals.",
                )),
            });
        }

        let ran = report.ado_job.as_ref().is_some_and(custom_job_ran);
        if ran && report.proposed_count == 0 {
            findings.push(Finding {
                category: String::from("safe_outputs"),
                severity: Severity::High,
                title: format!("Custom job ran without proposals for {}", report.tool),
                description: format!(
                    "The custom ADO job for '{}' started even though the Agent artifact contains no proposals for that tool.",
                    report.tool
                ),
                impact: Some(String::from(
                    "The compiler-generated proposal gate and the observed runtime state are inconsistent.",
                )),
            });
        }

        if ran
            && audit
                .detection_analysis
                .as_ref()
                .is_some_and(|analysis| !analysis.safe_to_process)
        {
            findings.push(Finding {
                category: String::from("safe_outputs"),
                severity: Severity::High,
                title: format!("Custom job ran after unsafe detection for {}", report.tool),
                description: format!(
                    "The custom ADO job for '{}' started even though threat detection marked the safe-output batch unsafe.",
                    report.tool
                ),
                impact: Some(String::from(
                    "A custom write-capable job appears to have bypassed the aggregate detection gate.",
                )),
            });
        }

        if ran
            && matches!(
                report.approval_path.as_deref(),
                Some("manual_review" | "post_review_dependency")
            )
            && !manual_review_succeeded(&audit.jobs)
        {
            findings.push(Finding {
                category: String::from("safe_outputs"),
                severity: Severity::High,
                title: format!("Custom reviewed job ran without approval for {}", report.tool),
                description: format!(
                    "The custom ADO job for '{}' is on the '{}' path, but no successful ManualReview job is present.",
                    report.tool,
                    report.approval_path.as_deref().unwrap_or_default()
                ),
                impact: Some(String::from(
                    "The observed execution state is inconsistent with the compiler's manual-review gate.",
                )),
            });
        }

        if report.proposed_count > 0
            && report.ado_job.is_none()
            && custom_job_should_have_appeared(audit, report)
        {
            findings.push(Finding {
                category: String::from("safe_outputs"),
                severity: Severity::High,
                title: format!("Expected custom job missing for {}", report.tool),
                description: format!(
                    "{} proposal(s) were recorded for custom tool '{}', detection allowed processing, but the expected custom ADO job{} is absent from the timeline.",
                    report.proposed_count,
                    report.tool,
                    report
                        .expected_job_id
                        .as_deref()
                        .map(|id| format!(" '{id}'"))
                        .unwrap_or_default()
                ),
                impact: Some(String::from(
                    "The custom proposals have no corresponding job-level execution outcome.",
                )),
            });
        }
    }
}

fn custom_job_should_have_appeared(audit: &AuditData, report: &CustomSafeOutputJobAudit) -> bool {
    if !audit
        .detection_analysis
        .as_ref()
        .is_some_and(|analysis| analysis.safe_to_process)
    {
        return false;
    }

    if matches!(
        report.approval_path.as_deref(),
        Some("manual_review" | "post_review_dependency")
    ) && !manual_review_succeeded(&audit.jobs)
    {
        return false;
    }
    if report.approval_path.is_none()
        && audit.jobs.iter().any(is_manual_review_job)
        && !manual_review_succeeded(&audit.jobs)
    {
        return false;
    }

    let Some(expected_job_id) = report.expected_job_id.as_deref() else {
        return true;
    };
    let Some(summary) = audit.pipeline_graph.as_ref().map(|graph| &graph.summary) else {
        return true;
    };
    let Some(expected) = summary.all_jobs().find(|job| job.id == expected_job_id) else {
        return true;
    };

    !expected.depends_on.iter().any(|dependency| {
        audit
            .jobs
            .iter()
            .find(|job| job.matches_ir_id(dependency))
            .is_some_and(job_blocks_downstream)
    })
}

fn job_blocks_downstream(job: &JobData) -> bool {
    let result = job.result.as_deref().unwrap_or_default();
    job.failed()
        || result.eq_ignore_ascii_case("skipped")
        || job.status.eq_ignore_ascii_case("skipped")
}

fn graph_job_id_for_tool(audit: &AuditData, tool: &str) -> Option<String> {
    graph_job_for_tool(audit, tool).map(|job| job.id.clone())
}

fn graph_job_for_tool<'a>(audit: &'a AuditData, tool: &str) -> Option<&'a JobSummary> {
    let summary = audit.pipeline_graph.as_ref().map(|graph| &graph.summary)?;
    graph_jobs_for_tool(summary, tool).first().copied()
}

fn unique_graph_display_name(audit: &AuditData, job_id: &str) -> Option<String> {
    let summary = audit.pipeline_graph.as_ref().map(|graph| &graph.summary)?;
    let job = summary.all_jobs().find(|job| job.id == job_id)?;
    (summary
        .all_jobs()
        .filter(|candidate| candidate.display_name == job.display_name)
        .count()
        == 1)
        .then(|| job.display_name.clone())
}

fn graph_jobs_for_tool<'a>(summary: &'a PipelineSummary, tool: &str) -> Vec<&'a JobSummary> {
    let base = custom_job_base(tool);
    summary
        .all_jobs()
        .filter(|job| job.id == base || job.id.ends_with(&format!("_{base}")))
        .collect()
}

fn approval_path_from_graph(audit: &AuditData, job_id: &str) -> Option<String> {
    let summary = audit.pipeline_graph.as_ref().map(|graph| &graph.summary)?;
    let job = summary.all_jobs().find(|job| job.id == job_id)?;
    if job.depends_on.iter().any(|id| is_manual_review_id(id)) {
        return Some(String::from("manual_review"));
    }
    if graph_has_manual_review_ancestor(summary, job_id, &mut HashSet::new()) {
        return Some(String::from("post_review_dependency"));
    }
    Some(String::from("automatic"))
}

fn graph_has_manual_review_ancestor(
    summary: &PipelineSummary,
    job_id: &str,
    visited: &mut HashSet<String>,
) -> bool {
    if !visited.insert(job_id.to_string()) {
        return false;
    }
    let Some(job) = summary.all_jobs().find(|job| job.id == job_id) else {
        return false;
    };
    job.depends_on.iter().any(|dependency| {
        is_manual_review_id(dependency)
            || graph_has_manual_review_ancestor(summary, dependency, visited)
    })
}

fn matching_timeline_jobs<'a>(
    jobs: &'a [JobData],
    tool: &str,
    expected_job_id: Option<&str>,
    expected_display_name: Option<&str>,
) -> Vec<&'a JobData> {
    let matches = jobs
        .iter()
        .filter(|job| job_matches_tool(job, tool, expected_job_id))
        .collect::<Vec<_>>();
    if !matches.is_empty() {
        return matches;
    }
    let generated = custom_job_base(tool);
    let id_name_matches = jobs
        .iter()
        .filter(|job| {
            expected_job_id
                .is_some_and(|expected| candidate_matches_job_id(&job.name, expected))
                || candidate_matches_job_id(&job.name, &generated)
        })
        .collect::<Vec<_>>();
    if id_name_matches.len() == 1 {
        return id_name_matches;
    }
    if id_name_matches.len() > 1 {
        return Vec::new();
    }
    let Some(expected_display_name) = expected_display_name else {
        return matches;
    };
    let display_matches = jobs
        .iter()
        .filter(|job| job.name == expected_display_name)
        .collect::<Vec<_>>();
    if display_matches.len() == 1 {
        display_matches
    } else {
        Vec::new()
    }
}

fn job_matches_tool(job: &JobData, tool: &str, expected_job_id: Option<&str>) -> bool {
    let generated = custom_job_base(tool);
    [
        job.timeline_ref_name.as_deref(),
        job.timeline_identifier.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|candidate| {
        expected_job_id.is_some_and(|expected| candidate_matches_job_id(candidate, expected))
            || candidate_matches_job_id(candidate, &generated)
    })
}

fn candidate_matches_job_id(candidate: &str, expected: &str) -> bool {
    let candidate = candidate.trim();
    let candidate = candidate
        .rsplit_once('.')
        .filter(|(prefix, _)| !prefix.contains('.'))
        .map_or(candidate, |(_, suffix)| suffix);
    candidate == expected || candidate.ends_with(&format!("_{expected}"))
}

fn custom_ado_job_from_timeline(job: &JobData) -> CustomSafeOutputAdoJob {
    CustomSafeOutputAdoJob {
        record_id: job.timeline_record_id.clone(),
        identifier: job.timeline_identifier.clone(),
        ref_name: job.timeline_ref_name.clone(),
        name: job.name.clone(),
        status: job.status.clone(),
        result: job.result.clone(),
        duration: job.duration.clone(),
        started_at: job.started_at.clone(),
        finished_at: job.finished_at.clone(),
    }
}

fn custom_job_ran(job: &CustomSafeOutputAdoJob) -> bool {
    job.started_at.is_some()
}

fn manual_review_succeeded(jobs: &[JobData]) -> bool {
    jobs.iter().any(|job| {
        is_manual_review_job(job)
            && job
                .result
                .as_deref()
                .is_some_and(|result| result.eq_ignore_ascii_case("succeeded"))
    })
}

fn is_manual_review_job(job: &JobData) -> bool {
    job.name.eq_ignore_ascii_case("Manual Review")
        || [
            Some(job.name.as_str()),
            job.timeline_ref_name.as_deref(),
            job.timeline_identifier.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(is_manual_review_id)
}

fn is_manual_review_id(candidate: &str) -> bool {
    candidate == "ManualReview"
        || candidate.ends_with("_ManualReview")
        || candidate.ends_with(".ManualReview")
}

fn custom_job_base(tool: &str) -> String {
    format!("Custom_{}", ado_identifier_suffix(tool))
}

fn ado_identifier_suffix(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
    {
        out
    } else {
        format!("_{out}")
    }
}

fn stable_ado_job_identity_matches(job: &CustomSafeOutputAdoJob, expected: &str) -> Option<bool> {
    let candidates = [job.ref_name.as_deref(), job.identifier.as_deref()]
        .into_iter()
        .flatten()
        .filter(|candidate| !candidate.trim().is_empty())
        .collect::<Vec<_>>();
    (!candidates.is_empty()).then(|| {
        candidates
            .into_iter()
            .any(|candidate| candidate_matches_job_id(candidate, expected))
    })
}

fn best_ado_job_identity(job: &CustomSafeOutputAdoJob) -> String {
    job.ref_name
        .as_deref()
        .or(job.identifier.as_deref())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&job.name)
        .to_string()
}

async fn load_proposal_counts(download_root: &Path) -> anyhow::Result<BTreeMap<String, u64>> {
    let Some(path) = find_proposals_file(download_root).await? else {
        return Ok(BTreeMap::new());
    };
    let mut counts = BTreeMap::new();
    for value in read_ndjson_file(&path).await? {
        let Some(tool) = value
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|tool| !tool.is_empty())
        else {
            continue;
        };
        *counts.entry(tool.to_string()).or_default() += 1;
    }
    Ok(counts)
}

async fn find_proposals_file(download_root: &Path) -> anyhow::Result<Option<PathBuf>> {
    for prefix in ["agent_outputs", "analyzed_outputs"] {
        let Some(directory) = latest_artifact_dir(download_root, prefix).await? else {
            continue;
        };
        for candidate in [
            directory.join("staging").join(SAFE_OUTPUT_FILENAME),
            directory.join(SAFE_OUTPUT_FILENAME),
        ] {
            if is_file(&candidate).await {
                return Ok(Some(candidate));
            }
        }
    }
    Ok(None)
}

async fn find_metadata_file(
    download_root: &Path,
    file_name: &str,
) -> anyhow::Result<Option<PathBuf>> {
    for prefix in ["agent_outputs", "analyzed_outputs"] {
        let Some(directory) = latest_artifact_dir(download_root, prefix).await? else {
            continue;
        };
        for candidate in [
            directory.join("staging").join(file_name),
            directory.join(file_name),
        ] {
            if is_file(&candidate).await {
                return Ok(Some(candidate));
            }
        }
    }
    Ok(None)
}

async fn load_aw_info(download_root: &Path) -> anyhow::Result<Option<AwInfo>> {
    let Some(path) = find_metadata_file(download_root, "aw_info.json").await? else {
        return Ok(None);
    };
    let contents = tokio::fs::read_to_string(&path)
        .await
        .with_context(|| format!("Failed to read aw_info file {}", path.display()))?;
    serde_json::from_str(&contents)
        .with_context(|| format!("Failed to parse aw_info file {}", path.display()))
        .map(Some)
}

async fn latest_artifact_dir(root: &Path, prefix: &str) -> anyhow::Result<Option<PathBuf>> {
    let mut entries = match tokio::fs::read_dir(root).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed to read directory {}", root.display()));
        }
    };
    let mut matches = Vec::new();
    while let Some(entry) = entries
        .next_entry()
        .await
        .with_context(|| format!("Failed to iterate {}", root.display()))?
    {
        if !entry
            .file_type()
            .await
            .with_context(|| format!("Failed to inspect {}", entry.path().display()))?
            .is_dir()
        {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == prefix || name.starts_with(&format!("{prefix}_")) {
            matches.push((name, entry.path()));
        }
    }
    matches.sort_by(|(left, _), (right, _)| crate::audit::cmp_numeric_suffix(left, right));
    Ok(matches.pop().map(|(_, path)| path))
}

async fn is_file(path: &Path) -> bool {
    tokio::fs::metadata(path)
        .await
        .map(|metadata| metadata.is_file())
        .unwrap_or(false)
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::model::{
        DetectionAnalysis, DetectionThreats, OverviewData, PipelineGraphSection,
    };
    use crate::compile::ir::summary::{
        GraphSummary, JobSummary, PipelineBodySummary, PipelineSummary, PoolSummary,
    };
    use serde_json::json;
    use tempfile::TempDir;

    fn completed_job(name: &str, ref_name: &str) -> JobData {
        JobData {
            name: name.to_string(),
            status: String::from("completed"),
            result: Some(String::from("succeeded")),
            duration: Some(String::from("0m 2s")),
            started_at: Some(String::from("2026-07-29T12:00:00Z")),
            finished_at: Some(String::from("2026-07-29T12:00:02Z")),
            timeline_record_id: Some(format!("record-{ref_name}")),
            timeline_identifier: Some(format!("identifier-{ref_name}")),
            timeline_ref_name: Some(ref_name.to_string()),
            ..Default::default()
        }
    }

    fn safe_detection() -> Option<DetectionAnalysis> {
        Some(DetectionAnalysis {
            threats: DetectionThreats::default(),
            reasons: Vec::new(),
            safe_to_process: true,
            verdict_path: None,
        })
    }

    fn graph_with_reviewed_custom_job() -> PipelineGraphSection {
        let jobs = vec![
            JobSummary {
                id: String::from("ManualReview"),
                stage: None,
                display_name: String::from("Manual Review"),
                depends_on: vec![String::from("Detection")],
                condition: None,
                pool: PoolSummary::Server,
                steps: Vec::new(),
            },
            JobSummary {
                id: String::from("Custom_notify_team"),
                stage: None,
                display_name: String::from("Notify team"),
                depends_on: vec![String::from("ManualReview")],
                condition: None,
                pool: PoolSummary::Named {
                    name: String::from("safe-outputs"),
                    image: None,
                    os: None,
                    demands: Vec::new(),
                },
                steps: Vec::new(),
            },
        ];
        PipelineGraphSection {
            source_path: String::from("agents/test.md"),
            summary: PipelineSummary {
                schema_version: 1,
                name: String::from("test"),
                shape: String::from("standalone"),
                body: PipelineBodySummary::Jobs { jobs },
                graph: GraphSummary {
                    step_locations: Vec::new(),
                    job_edges: Vec::new(),
                    stage_edges: Vec::new(),
                    outputs_needing_is_output: Vec::new(),
                },
            },
        }
    }

    #[tokio::test]
    async fn correlates_proposals_metadata_and_timeline_at_job_level() {
        let temp_dir = TempDir::new().expect("temp dir");
        let schema_digest = crate::hash::sha256_hex(
            &serde_json::to_vec(&json!({"type": "object"})).expect("serialize schema"),
        );
        write_custom_artifacts(
            &temp_dir,
            &[json!({"name": "notify-team", "message": "hello"})],
            &[json!({
                "name": "notify-team",
                "description": "Notify",
                "inputSchema": {"type": "object"},
                "max": 1,
                "output": "Notification proposal accepted."
            })],
        )
        .await;

        let mut audit = AuditData {
            overview: OverviewData {
                aw_info: Some(AwInfo {
                    custom_components: vec![ComponentProvenance {
                        tool: String::from("notify-team"),
                        source: String::from("org/repo/components/notify"),
                        requested_ref: Some(String::from("refs/tags/v1")),
                        sha: String::from("0123456789abcdef0123456789abcdef01234567"),
                        manifest_digest: String::from("sha256:manifest"),
                        schema_digest,
                    }],
                    custom_jobs: vec![CustomSafeOutputJobMetadata {
                        tool: String::from("notify-team"),
                        job_id: Some(String::from("Custom_notify_team")),
                        approval_path: Some(String::from("manual_review")),
                        staged_requested: Some(true),
                    }],
                    ..Default::default()
                }),
                ..Default::default()
            },
            detection_analysis: safe_detection(),
            jobs: vec![
                completed_job("Manual Review", "ManualReview"),
                completed_job("Notify team", "Custom_notify_team"),
            ],
            ..Default::default()
        };

        populate_custom_safe_output_jobs(&mut audit, temp_dir.path())
            .await
            .expect("populate custom jobs");

        assert_eq!(audit.custom_safe_output_jobs.len(), 1);
        let report = &audit.custom_safe_output_jobs[0];
        assert_eq!(report.tool, "notify-team");
        assert_eq!(report.proposed_count, 1);
        assert_eq!(
            report.expected_job_id.as_deref(),
            Some("Custom_notify_team")
        );
        assert_eq!(report.approval_path.as_deref(), Some("manual_review"));
        assert_eq!(report.staged_requested, Some(true));
        assert_eq!(
            report.proposal_time_acknowledgement.as_deref(),
            Some("Notification proposal accepted.")
        );
        assert_eq!(
            report
                .component_provenance
                .as_ref()
                .and_then(|component| component.requested_ref.as_deref()),
            Some("refs/tags/v1")
        );
        let ado_job = report.ado_job.as_ref().expect("correlated ADO job");
        assert_eq!(ado_job.ref_name.as_deref(), Some("Custom_notify_team"));
        assert_eq!(ado_job.result.as_deref(), Some("succeeded"));
        assert!(audit.key_findings.is_empty());
    }

    #[tokio::test]
    async fn graph_derives_expected_identity_and_review_path() {
        let temp_dir = TempDir::new().expect("temp dir");
        write_custom_artifacts(
            &temp_dir,
            &[json!({"name": "notify-team"})],
            &[json!({
                "name": "notify-team",
                "description": "Notify",
                "inputSchema": {"type": "object"},
                "max": 1
            })],
        )
        .await;

        let mut audit = AuditData {
            detection_analysis: safe_detection(),
            pipeline_graph: Some(graph_with_reviewed_custom_job()),
            jobs: vec![
                completed_job("Manual Review", "ManualReview"),
                JobData {
                    name: String::from("Notify team"),
                    status: String::from("completed"),
                    result: Some(String::from("succeeded")),
                    started_at: Some(String::from("2026-07-29T12:00:00Z")),
                    finished_at: Some(String::from("2026-07-29T12:00:02Z")),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        populate_custom_safe_output_jobs(&mut audit, temp_dir.path())
            .await
            .expect("populate custom jobs");

        let report = &audit.custom_safe_output_jobs[0];
        assert_eq!(
            report.expected_job_id.as_deref(),
            Some("Custom_notify_team")
        );
        assert_eq!(report.approval_path.as_deref(), Some("manual_review"));
        assert_eq!(
            report.ado_job.as_ref().map(|job| job.name.as_str()),
            Some("Notify team")
        );
        assert!(audit.key_findings.is_empty(), "{:?}", audit.key_findings);
    }

    #[tokio::test]
    async fn duplicate_display_names_do_not_correlate_cached_custom_jobs() {
        let temp_dir = TempDir::new().expect("temp dir");
        write_custom_artifacts(
            &temp_dir,
            &[
                json!({"name": "notify-team"}),
                json!({"name": "publish-summary"}),
            ],
            &[
                json!({
                    "name": "notify-team",
                    "description": "Notify",
                    "inputSchema": {"type": "object"},
                    "max": 1
                }),
                json!({
                    "name": "publish-summary",
                    "description": "Publish",
                    "inputSchema": {"type": "object"},
                    "max": 1
                }),
            ],
        )
        .await;

        let custom_job = |id: &str| JobSummary {
            id: id.to_string(),
            stage: None,
            display_name: String::from("Custom safe output: notify-team"),
            depends_on: vec![String::from("Detection")],
            condition: None,
            pool: PoolSummary::Named {
                name: String::from("safe-outputs"),
                image: None,
                os: None,
                demands: Vec::new(),
            },
            steps: Vec::new(),
        };
        let mut audit = AuditData {
            detection_analysis: safe_detection(),
            pipeline_graph: Some(PipelineGraphSection {
                source_path: String::from("agents/test.md"),
                summary: PipelineSummary {
                    schema_version: 1,
                    name: String::from("test"),
                    shape: String::from("standalone"),
                    body: PipelineBodySummary::Jobs {
                        jobs: vec![
                            custom_job("Custom_notify_team"),
                            custom_job("Custom_publish_summary"),
                        ],
                    },
                    graph: GraphSummary {
                        step_locations: Vec::new(),
                        job_edges: Vec::new(),
                        stage_edges: Vec::new(),
                        outputs_needing_is_output: Vec::new(),
                    },
                },
            }),
            jobs: vec![
                JobData {
                    name: String::from("Custom safe output: notify-team"),
                    status: String::from("completed"),
                    result: Some(String::from("succeeded")),
                    ..Default::default()
                },
                JobData {
                    name: String::from("Custom safe output: notify-team"),
                    status: String::from("completed"),
                    result: Some(String::from("failed")),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        populate_custom_safe_output_jobs(&mut audit, temp_dir.path())
            .await
            .expect("populate custom jobs");

        assert_eq!(audit.custom_safe_output_jobs.len(), 2);
        assert!(
            audit
                .custom_safe_output_jobs
                .iter()
                .all(|report| report.ado_job.is_none())
        );
    }

    #[tokio::test]
    async fn legacy_cached_generated_job_name_correlates_when_unique() {
        let temp_dir = TempDir::new().expect("temp dir");
        write_custom_artifacts(
            &temp_dir,
            &[json!({"name": "notify-team"})],
            &[json!({
                "name": "notify-team",
                "description": "Notify",
                "inputSchema": {"type": "object"},
                "max": 1
            })],
        )
        .await;
        let mut audit = AuditData {
            detection_analysis: safe_detection(),
            jobs: vec![JobData {
                name: String::from("Custom_notify_team"),
                status: String::from("completed"),
                result: Some(String::from("succeeded")),
                started_at: Some(String::from("2026-07-29T12:00:00Z")),
                ..Default::default()
            }],
            overview: OverviewData {
                aw_info: Some(AwInfo {
                    custom_jobs: vec![CustomSafeOutputJobMetadata {
                        tool: String::from("notify-team"),
                        job_id: Some(String::from("Custom_notify_team")),
                        approval_path: Some(String::from("automatic")),
                        staged_requested: None,
                    }],
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        populate_custom_safe_output_jobs(&mut audit, temp_dir.path())
            .await
            .expect("populate custom jobs");

        assert_eq!(
            audit.custom_safe_output_jobs[0]
                .ado_job
                .as_ref()
                .map(|job| job.name.as_str()),
            Some("Custom_notify_team")
        );
    }

    #[tokio::test]
    async fn graph_identity_and_approval_override_conflicting_aw_info_claims() {
        let temp_dir = TempDir::new().expect("temp dir");
        write_custom_artifacts(
            &temp_dir,
            &[json!({"name": "notify-team"})],
            &[json!({
                "name": "notify-team",
                "description": "Notify",
                "inputSchema": {"type": "object"},
                "max": 1
            })],
        )
        .await;

        let mut audit = AuditData {
            overview: OverviewData {
                aw_info: Some(AwInfo {
                    custom_jobs: vec![CustomSafeOutputJobMetadata {
                        tool: String::from("notify-team"),
                        job_id: Some(String::from("Spoofed_job")),
                        approval_path: Some(String::from("post_review_dependency")),
                        staged_requested: None,
                    }],
                    ..Default::default()
                }),
                ..Default::default()
            },
            detection_analysis: safe_detection(),
            pipeline_graph: Some(graph_with_reviewed_custom_job()),
            jobs: vec![
                completed_job("Manual Review", "ManualReview"),
                completed_job("Notify team", "Custom_notify_team"),
            ],
            ..Default::default()
        };

        populate_custom_safe_output_jobs(&mut audit, temp_dir.path())
            .await
            .expect("populate custom jobs");

        let report = &audit.custom_safe_output_jobs[0];
        assert_eq!(
            report.expected_job_id.as_deref(),
            Some("Custom_notify_team")
        );
        assert_eq!(report.approval_path.as_deref(), Some("manual_review"));
        assert!(audit.key_findings.iter().any(|finding| {
            finding.title == "Custom job metadata identity mismatch for notify-team"
        }));
        assert!(audit.key_findings.iter().any(|finding| {
            finding.title == "Custom job metadata approval mismatch for notify-team"
        }));
    }

    #[tokio::test]
    async fn missing_expected_job_produces_finding_when_detection_is_safe() {
        let temp_dir = TempDir::new().expect("temp dir");
        write_custom_artifacts(
            &temp_dir,
            &[json!({"name": "notify"})],
            &[json!({
                "name": "notify",
                "description": "Notify",
                "inputSchema": {"type": "object"},
                "max": 1
            })],
        )
        .await;
        let mut audit = AuditData {
            detection_analysis: safe_detection(),
            ..Default::default()
        };

        populate_custom_safe_output_jobs(&mut audit, temp_dir.path())
            .await
            .expect("populate custom jobs");

        assert!(audit.key_findings.iter().any(|finding| {
            finding.title == "Expected custom job missing for notify"
                && finding.severity == Severity::High
        }));
    }

    #[tokio::test]
    async fn schema_provenance_mismatch_produces_finding() {
        let temp_dir = TempDir::new().expect("temp dir");
        write_custom_artifacts(
            &temp_dir,
            &[],
            &[json!({
                "name": "notify",
                "description": "Notify",
                "inputSchema": {"type": "object"},
                "max": 1
            })],
        )
        .await;
        let mut audit = AuditData {
            overview: OverviewData {
                aw_info: Some(AwInfo {
                    custom_components: vec![ComponentProvenance {
                        tool: String::from("notify"),
                        source: String::from("org/repo/notify"),
                        requested_ref: None,
                        sha: String::from("0123456789abcdef0123456789abcdef01234567"),
                        manifest_digest: String::from("manifest"),
                        schema_digest: String::from("incorrect-schema-digest"),
                    }],
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        populate_custom_safe_output_jobs(&mut audit, temp_dir.path())
            .await
            .expect("populate custom jobs");

        assert!(audit.key_findings.iter().any(|finding| {
            finding.title == "Custom component provenance mismatch for notify"
                && finding.severity == Severity::High
        }));
    }

    #[tokio::test]
    async fn unsafe_detection_with_started_custom_job_is_impossible_state() {
        let temp_dir = TempDir::new().expect("temp dir");
        write_custom_artifacts(
            &temp_dir,
            &[json!({"name": "notify"})],
            &[json!({
                "name": "notify",
                "description": "Notify",
                "inputSchema": {"type": "object"},
                "max": 1
            })],
        )
        .await;
        let mut audit = AuditData {
            detection_analysis: Some(DetectionAnalysis {
                threats: DetectionThreats {
                    prompt_injection: true,
                    ..Default::default()
                },
                reasons: vec![String::from("unsafe")],
                safe_to_process: false,
                verdict_path: None,
            }),
            jobs: vec![completed_job("Notify", "Custom_notify")],
            ..Default::default()
        };

        populate_custom_safe_output_jobs(&mut audit, temp_dir.path())
            .await
            .expect("populate custom jobs");

        assert!(audit.key_findings.iter().any(|finding| {
            finding.title == "Custom job ran after unsafe detection for notify"
                && finding.severity == Severity::High
        }));
        assert!(
            !audit
                .key_findings
                .iter()
                .any(|finding| finding.title == "Expected custom job missing for notify")
        );
    }

    #[tokio::test]
    async fn unstarted_custom_job_does_not_trigger_gate_bypass_findings() {
        let temp_dir = TempDir::new().expect("temp dir");
        write_custom_artifacts(
            &temp_dir,
            &[json!({"name": "notify"})],
            &[json!({
                "name": "notify",
                "description": "Notify",
                "inputSchema": {"type": "object"},
                "max": 1
            })],
        )
        .await;
        let mut audit = AuditData {
            overview: OverviewData {
                aw_info: Some(AwInfo {
                    custom_jobs: vec![CustomSafeOutputJobMetadata {
                        tool: String::from("notify"),
                        job_id: Some(String::from("Custom_notify")),
                        approval_path: Some(String::from("manual_review")),
                        staged_requested: None,
                    }],
                    ..Default::default()
                }),
                ..Default::default()
            },
            detection_analysis: Some(DetectionAnalysis {
                threats: DetectionThreats {
                    prompt_injection: true,
                    ..Default::default()
                },
                reasons: vec![String::from("unsafe")],
                safe_to_process: false,
                verdict_path: None,
            }),
            jobs: vec![JobData {
                name: String::from("Notify"),
                status: String::from("pending"),
                result: Some(String::from("canceled")),
                timeline_ref_name: Some(String::from("Custom_notify")),
                ..Default::default()
            }],
            ..Default::default()
        };

        populate_custom_safe_output_jobs(&mut audit, temp_dir.path())
            .await
            .expect("populate custom jobs");

        assert!(!audit.key_findings.iter().any(|finding| {
            finding.title == "Custom job ran after unsafe detection for notify"
                || finding.title == "Custom reviewed job ran without approval for notify"
        }));
    }

    async fn write_custom_artifacts(
        temp_dir: &TempDir,
        proposals: &[serde_json::Value],
        configs: &[serde_json::Value],
    ) {
        let staging = temp_dir.path().join("agent_outputs_42").join("staging");
        tokio::fs::create_dir_all(&staging)
            .await
            .expect("create staging");
        let proposal_text = proposals
            .iter()
            .map(serde_json::Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        tokio::fs::write(
            staging.join(SAFE_OUTPUT_FILENAME),
            format!("{proposal_text}\n"),
        )
        .await
        .expect("write proposals");
        tokio::fs::write(
            staging.join(CUSTOM_TOOLS_FILENAME),
            serde_json::to_vec(configs).expect("serialize configs"),
        )
        .await
        .expect("write custom tools");
    }
}
