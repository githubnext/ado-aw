#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

fn is_zero_f64(value: &f64) -> bool {
    *value == 0.0
}

/// Top-level audit report for a single Azure DevOps build.
///
/// This model is populated from build metadata plus downloaded stage artifacts such as
/// `agent_outputs_<BuildId>`, `analyzed_outputs_<BuildId>`, and `safe_outputs`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AuditData {
    /// High-level build and pipeline metadata resolved from Azure DevOps APIs and staged metadata files.
    pub overview: OverviewData,
    /// Task-domain classification derived from audit heuristics over the run's prompts and outputs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_domain: Option<TaskDomainInfo>,
    /// Behavior fingerprint information derived from analyzer heuristics over the run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub behavior_fingerprint: Option<BehaviorFingerprint>,
    /// Agentic assessments emitted by higher-level audit heuristics.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub agentic_assessments: Vec<AgenticAssessment>,
    /// Aggregate numeric metrics derived from OTel and audit processing.
    pub metrics: MetricsData,
    /// Important findings synthesized from analyzer output.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub key_findings: Vec<Finding>,
    /// Recommended next actions derived from the audit findings.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub recommendations: Vec<Recommendation>,
    /// Optional derived performance metrics computed from token, cost, and tool usage data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub performance_metrics: Option<PerformanceMetrics>,
    /// Engine configuration captured from compiled metadata and runtime emission.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine_config: Option<AuditEngineConfig>,
    /// Rollup of proposed, executed, and dropped safe outputs for the build.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safe_output_summary: Option<SafeOutputSummary>,
    /// Per-item safe-output execution outcomes emitted by the ADO SafeOutputs stage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safe_output_execution: Option<SafeOutputExecution>,
    /// Aggregate rollup of safe outputs rejected before or during execution.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejected_safe_outputs: Option<RejectedSafeOutputsRollup>,
    /// Threat-detection verdict information from `analyzed_outputs_<BuildId>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detection_analysis: Option<DetectionAnalysis>,
    /// MCP server reliability and call health derived from gateway logs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_server_health: Option<MCPServerHealth>,
    /// Job-level status data derived from the Azure DevOps build timeline.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub jobs: Vec<JobData>,
    /// Files downloaded while assembling the audit input set.
    #[serde(default)]
    pub downloaded_files: Vec<FileInfo>,
    /// Missing-tool reports captured from safe-output or MCP artifacts.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub missing_tools: Vec<MissingToolReport>,
    /// Missing-data reports captured from safe-output or MCP artifacts.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub missing_data: Vec<MissingDataReport>,
    /// No-op reports emitted by runtime tools during the build.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub noops: Vec<NoopReport>,
    /// MCP failure reports derived from gateway or tool execution artifacts.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub mcp_failures: Vec<MCPFailureReport>,
    /// Firewall-domain analysis derived from AWF firewall logs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub firewall_analysis: Option<FirewallAnalysis>,
    /// Policy-rule analysis derived from AWF policy artifacts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_analysis: Option<PolicyAnalysis>,
    /// Non-fatal or fatal errors encountered while auditing or discovered in artifacts.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub errors: Vec<ErrorInfo>,
    /// Warning rows surfaced during audit processing.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub warnings: Vec<ErrorInfo>,
    /// High-level tool usage rollups derived from runtime telemetry.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tool_usage: Vec<ToolUsageInfo>,
    /// MCP-specific tool usage rollups derived from MCP gateway logs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_tool_usage: Option<MCPToolUsageData>,
    /// Created external items reported by successful safe-output execution.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub created_items: Vec<CreatedItemReport>,
}

/// Overview metadata for the audited build.
///
/// This is sourced from Azure DevOps build APIs, timeline data, and `staging/aw_info.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OverviewData {
    /// Azure DevOps build identifier.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub build_id: u64,
    /// Azure DevOps pipeline definition name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pipeline_name: String,
    /// Build lifecycle status such as `completed` or `inProgress`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub status: String,
    /// Final build result using Azure DevOps terminology.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// Build creation timestamp from the Azure DevOps build record.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    /// Build start timestamp from the Azure DevOps build record or timeline.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    /// Build completion timestamp from the Azure DevOps build record or timeline.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    /// Human-readable build duration derived from build timestamps.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<String>,
    /// Source branch recorded for the audited run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_branch: Option<String>,
    /// Source commit or version recorded for the audited run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_version: Option<String>,
    /// Human-facing URL for the audited build.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Local path where build logs or downloaded artifacts were stored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logs_path: Option<String>,
    /// Runtime-emitted AW metadata from `staging/aw_info.json`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aw_info: Option<AwInfo>,
}

/// Runtime-emitted agentic workflow metadata.
///
/// This is read from `staging/aw_info.json`, which mirrors the compiled marker metadata plus runtime context.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AwInfo {
    /// Configured engine name for the run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    /// Model identifier used by the agent runtime.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Agent name emitted by the compiled workflow metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    /// Source markdown path for the compiled workflow.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Compile target for the workflow, such as `standalone` or `stage`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Compiler version that produced the workflow.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compiler_version: Option<String>,
}

/// Aggregate numeric metrics for the audited run.
///
/// These values are typically sourced from OTel logs plus audit-time counting of warnings and errors.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MetricsData {
    /// Total tokens consumed by the run.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub token_usage: u64,
    /// Effective billed or normalized tokens used for costing.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub effective_tokens: u64,
    /// Estimated run cost in provider currency units.
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub estimated_cost: f64,
    /// Total model turns captured for the run.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub turns: u64,
    /// Number of error rows captured in the audit.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub error_count: u64,
    /// Number of warning rows captured in the audit.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub warning_count: u64,
}

/// A notable audit finding synthesized from one or more analyzer results.
///
/// Findings are rendered in the report and originate from analyzer heuristics over downloaded artifacts.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Finding {
    /// Logical finding category such as `security`, `cost`, or `tooling`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub category: String,
    /// Severity assigned by the audit heuristics.
    pub severity: Severity,
    /// Short human-readable title for the finding.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    /// Longer explanation of the finding.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Optional impact statement explaining why the finding matters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub impact: Option<String>,
}

/// Severity assigned to an audit finding.
///
/// This matches the lowercase wire format used by gh-aw-compatible JSON consumers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Critical issue requiring immediate attention.
    Critical,
    /// High-severity issue with significant impact.
    High,
    /// Medium-severity issue with moderate impact.
    Medium,
    /// Low-severity issue with limited impact.
    Low,
    /// Informational observation.
    #[default]
    Info,
}

/// Recommended follow-up action emitted by the audit.
///
/// Recommendations are synthesized from findings and intended for humans or automation consuming the report.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Recommendation {
    /// Recommendation priority, typically `high`, `medium`, or `low`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub priority: String,
    /// Recommended action to take.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub action: String,
    /// Reason the recommendation was emitted.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    /// Optional example command, config, or remediation snippet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<String>,
}

/// Derived performance-oriented metrics for the run.
///
/// These values are computed from audit data such as token usage, tool calls, and network activity.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PerformanceMetrics {
    /// Throughput estimate derived from tokens and duration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens_per_minute: Option<f64>,
    /// Human-readable description of cost efficiency.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_efficiency: Option<String>,
    /// Most frequently used tool name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub most_used_tool: Option<String>,
    /// Number of observed network requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network_requests: Option<u64>,
}

/// Engine configuration recorded for the audited run.
///
/// This is sourced from compiled workflow metadata and runtime-emitted AW info.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AuditEngineConfig {
    /// Engine identifier such as `copilot`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub engine: String,
    /// Model identifier configured for the run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Engine version string, when emitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Timeout configured for the run in minutes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_minutes: Option<u64>,
}

/// Job-level status information for one stage in the build timeline.
///
/// This is derived from Azure DevOps timeline records for the audited build.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct JobData {
    /// Job display name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Job status such as `completed` or `inProgress`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub status: String,
    /// Final job result using Azure DevOps terminology.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// Human-readable job duration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<String>,
    /// Job start timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    /// Job finish timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
}

/// Metadata about a file downloaded while assembling the audit.
///
/// These rows are produced by the artifact download phase for traceability and caching.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FileInfo {
    /// Relative or absolute file path on disk.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,
    /// File size in bytes.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub size_bytes: u64,
    /// Optional SHA-256 digest of the downloaded file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// Report describing a requested tool that was unavailable to the agent.
///
/// These rows are typically sourced from missing-tool safe-output or MCP artifacts.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MissingToolReport {
    /// Tool name, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// Optional contextual identifier for where the problem occurred.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// Optional human-readable reason for the report.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Optional timestamp for when the report was emitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    /// Forward-compatible payload preserved from the source artifact.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub extra: Value,
}

/// Report describing required data that was unavailable to the agent.
///
/// These rows are typically sourced from missing-data safe-output or MCP artifacts.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MissingDataReport {
    /// Tool name associated with the missing data, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// Optional contextual identifier for where the problem occurred.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// Optional human-readable reason for the report.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Optional timestamp for when the report was emitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    /// Forward-compatible payload preserved from the source artifact.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub extra: Value,
}

/// Report describing a tool invocation that intentionally performed no action.
///
/// These rows are typically sourced from noop safe-output artifacts.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NoopReport {
    /// Tool name, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// Optional contextual identifier for where the noop occurred.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// Optional human-readable reason for the noop.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Optional timestamp for when the noop was emitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    /// Forward-compatible payload preserved from the source artifact.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub extra: Value,
}

/// Report describing a failed MCP interaction.
///
/// These rows are sourced from MCP gateway logs or failure artifacts emitted during the run.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MCPFailureReport {
    /// Tool name involved in the failure, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// Optional contextual identifier for where the failure occurred.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// Optional human-readable failure reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Optional timestamp for when the failure was emitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    /// Forward-compatible payload preserved from the source artifact.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub extra: Value,
}

/// Firewall-domain activity analysis for the audited run.
///
/// This section is derived from AWF firewall or proxy logs in the agent artifact set.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FirewallAnalysis {
    /// Per-domain firewall statistics.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub domains: Vec<DomainStat>,
    /// Total observed firewall requests.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub total_requests: u64,
    /// Number of allowed requests.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub allowed_count: u64,
    /// Number of denied requests.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub denied_count: u64,
}

/// Firewall statistics for a single domain.
///
/// These values are derived from AWF firewall request logs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DomainStat {
    /// Domain name observed in firewall logs.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub domain: String,
    /// Aggregate status such as `allowed`, `denied`, or `mixed`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub status: String,
    /// Number of observed requests for the domain.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub request_count: u64,
    /// First-observed timestamp for the domain.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_seen: Option<String>,
    /// Last-observed timestamp for the domain.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<String>,
}

/// Policy-analysis summary for the audited run.
///
/// This section is derived from AWF policy manifests and audit logs when those artifacts are present.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PolicyAnalysis {
    /// Per-rule policy statistics.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub policies: Vec<PolicyRule>,
    /// Count of allow verdicts observed.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub allow_count: u64,
    /// Count of deny verdicts observed.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub deny_count: u64,
}

/// Hit statistics for a single policy rule.
///
/// These values are derived from AWF policy evaluation artifacts.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PolicyRule {
    /// Rule pattern or selector.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pattern: String,
    /// Final verdict such as `allow` or `deny`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub verdict: String,
    /// Number of observed hits for the rule.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub hit_count: u64,
}

/// Reliability summary for MCP servers observed during the run.
///
/// This section is derived from MCP gateway logs collected in the agent artifact set.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MCPServerHealth {
    /// Per-server call and error statistics.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub servers: Vec<MCPServerStats>,
}

/// Aggregate statistics for one MCP server.
///
/// These values are derived from MCP gateway request and error logs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MCPServerStats {
    /// MCP server name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Total number of calls routed to the server.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub total_calls: u64,
    /// Number of calls that failed.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub error_count: u64,
    /// Fraction of calls that failed.
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub error_rate: f64,
    /// Whether the server should be considered unreliable.
    #[serde(default)]
    pub unreliable: bool,
}

/// MCP-tool usage summary for the run.
///
/// This section is derived from MCP gateway logs and summarizes calls per tool.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MCPToolUsageData {
    /// Per-tool usage statistics.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<MCPToolSummary>,
}

/// Aggregate usage statistics for one MCP tool.
///
/// These values are derived from MCP gateway logs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MCPToolSummary {
    /// MCP tool name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Number of times the tool was called.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub call_count: u64,
    /// Number of failed tool calls.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub error_count: u64,
    /// Largest observed input payload size.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub max_input_size: u64,
    /// Largest observed output payload size.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub max_output_size: u64,
}

/// Aggregate usage information for any tool seen during the run.
///
/// This is derived from runtime telemetry such as OTel or analyzer-specific timing data.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolUsageInfo {
    /// Tool name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Number of times the tool was called.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub call_count: u64,
    /// Total observed duration in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_duration_ms: Option<u64>,
}

/// Error or warning entry captured during the audit.
///
/// These rows come from analyzer failures, audit warnings, or surfaced runtime diagnostics.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ErrorInfo {
    /// Source component that emitted the row.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    /// Human-readable error or warning message.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    /// Optional timestamp for the row.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}

/// External item created as a result of safe-output execution.
///
/// These rows are derived from successful Stage 3 execution results.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CreatedItemReport {
    /// Created item kind such as `pull_request` or `work_item`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    /// URL of the created item, when one exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Stable identifier of the created item, when one exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Human-readable title of the created item, when one exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// Aggregate safe-output counts for the audited build.
///
/// This summary is derived by correlating proposed, analyzed, and executed safe-output artifacts.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SafeOutputSummary {
    /// Number of safe outputs proposed by the agent.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub proposed_count: u64,
    /// Number of safe outputs executed successfully.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub executed_count: u64,
    /// Number of safe outputs rejected during execution.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub rejected_by_execution_count: u64,
    /// Number of safe outputs left unprocessed.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub not_processed_count: u64,
}

/// Per-item safe-output execution details for the build.
///
/// This section is sourced from execution manifests written by the SafeOutputs stage.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SafeOutputExecution {
    /// Itemized execution outcomes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<SafeOutputExecutionItem>,
}

/// Execution outcome for one safe-output proposal.
///
/// This row is derived by joining proposal, detection, and execution artifacts for a single proposal context.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SafeOutputExecutionItem {
    /// Optional proposal context used to correlate artifacts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// Safe-output tool name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tool: String,
    /// Final processing status for the proposal.
    pub status: SafeOutputStatus,
    /// Original proposal payload captured from the agent artifact.
    pub proposal: Value,
    /// Optional execution error string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Optional execution result payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Optional rejection reason emitted by detection or execution.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejection_reason: Option<String>,
    /// Whether the status applies to the entire batch rather than a single proposal.
    #[serde(default)]
    pub applies_to_whole_batch: bool,
}

/// Final processing status for a safe-output proposal.
///
/// The snake_case wire format matches the ADO audit JSON contract.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SafeOutputStatus {
    /// Proposal was executed successfully.
    #[default]
    Executed,
    /// Proposal was rejected while executing.
    RejectedByExecution,
    /// Proposal was not processed because aggregate threat detection gated the batch.
    NotProcessedDueToAggregateGate,
    /// Proposal was intentionally skipped.
    Skipped,
    /// Proposal could not be processed because execution budget was exhausted.
    BudgetExhausted,
}

/// Aggregate rollup of rejected safe outputs.
///
/// This section is derived from detection verdicts and execution results for JSON consumers that prefer summary data.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RejectedSafeOutputsRollup {
    /// Total number of rejected proposals.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub total_rejected: u64,
    /// Rejection counts grouped by reason string.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub by_reason: BTreeMap<String, u64>,
    /// Rejection counts grouped by threat kind.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub by_threat: BTreeMap<String, u64>,
}

/// Aggregate threat-detection verdict for the run.
///
/// This data is read from detection-stage artifacts such as `threat-analysis.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DetectionAnalysis {
    /// Threat flags reported by the detection stage.
    pub threats: DetectionThreats,
    /// Human-readable reasons emitted by threat detection.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<String>,
    /// Whether the run's safe outputs were considered safe to process.
    #[serde(default)]
    pub safe_to_process: bool,
    /// Optional path to the stored verdict artifact.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict_path: Option<String>,
}

/// Threat flags produced by the detection stage.
///
/// These booleans come from the aggregate threat-detection verdict emitted for the run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DetectionThreats {
    /// Whether prompt injection was detected.
    #[serde(default)]
    pub prompt_injection: bool,
    /// Whether a secret leak was detected.
    #[serde(default)]
    pub secret_leak: bool,
    /// Whether a malicious patch was detected.
    #[serde(default)]
    pub malicious_patch: bool,
}

/// Placeholder task-domain summary for the MVP audit contract.
///
/// This opaque section keeps the JSON shape compatible until richer heuristics land.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TaskDomainInfo {
    /// Human-readable summary of the inferred task domain.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    /// Opaque analyzer-specific payload for future expansion.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub data: Value,
}

/// Placeholder behavior-fingerprint summary for the MVP audit contract.
///
/// This opaque section keeps the JSON shape compatible until richer heuristics land.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BehaviorFingerprint {
    /// Human-readable summary of the inferred behavior pattern.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    /// Opaque analyzer-specific payload for future expansion.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub data: Value,
}

/// Placeholder agentic assessment summary for the MVP audit contract.
///
/// This opaque section keeps the JSON shape compatible until richer heuristics land.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgenticAssessment {
    /// Human-readable summary of the assessment.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    /// Opaque analyzer-specific payload for future expansion.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub data: Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn populated_audit_data() -> AuditData {
        let mut by_reason = BTreeMap::new();
        by_reason.insert(String::from("aggregate_gate"), 1);

        let mut by_threat = BTreeMap::new();
        by_threat.insert(String::from("prompt_injection"), 1);
        by_threat.insert(String::from("secret_leak"), 0);
        by_threat.insert(String::from("malicious_patch"), 0);

        AuditData {
            overview: OverviewData {
                build_id: 42,
                pipeline_name: String::from("agentic-pipeline"),
                status: String::from("completed"),
                result: Some(String::from("succeeded")),
                created_at: Some(String::from("2026-05-21T12:00:00Z")),
                started_at: Some(String::from("2026-05-21T12:01:00Z")),
                finished_at: Some(String::from("2026-05-21T12:06:00Z")),
                duration: Some(String::from("5m")),
                source_branch: Some(String::from("refs/heads/main")),
                source_version: Some(String::from("abcdef123456")),
                url: Some(String::from(
                    "https://dev.azure.com/example/project/_build/results?buildId=42",
                )),
                logs_path: Some(String::from("logs\\build-42")),
                aw_info: Some(AwInfo {
                    engine: Some(String::from("copilot")),
                    model: Some(String::from("gpt-5.4")),
                    agent_name: Some(String::from("agentic-auditor")),
                    source: Some(String::from("agents/security-scan.md")),
                    target: Some(String::from("standalone")),
                    compiler_version: Some(String::from("0.30.2")),
                }),
            },
            task_domain: Some(TaskDomainInfo {
                summary: String::from("Security review workflow"),
                data: json!({"domain": "security"}),
            }),
            behavior_fingerprint: Some(BehaviorFingerprint {
                summary: String::from("High tool usage with safe outputs"),
                data: json!({"pattern": "tool-heavy"}),
            }),
            agentic_assessments: vec![AgenticAssessment {
                summary: String::from("Agent produced actionable changes"),
                data: json!({"score": 0.92}),
            }],
            metrics: MetricsData {
                token_usage: 1200,
                effective_tokens: 1000,
                estimated_cost: 1.23,
                turns: 12,
                error_count: 1,
                warning_count: 2,
            },
            key_findings: vec![Finding {
                category: String::from("security"),
                severity: Severity::High,
                title: String::from("Detection gate tripped"),
                description: String::from("Threat detection blocked the safe-output batch."),
                impact: Some(String::from("No proposed changes were executed.")),
            }],
            recommendations: vec![Recommendation {
                priority: String::from("high"),
                action: String::from("Review the detection-stage verdict"),
                reason: String::from("The aggregate gate prevented execution."),
                example: Some(String::from(
                    "Inspect analyzed_outputs_42\\threat-analysis.json",
                )),
            }],
            performance_metrics: Some(PerformanceMetrics {
                tokens_per_minute: Some(240.0),
                cost_efficiency: Some(String::from("moderate")),
                most_used_tool: Some(String::from("edit")),
                network_requests: Some(18),
            }),
            engine_config: Some(AuditEngineConfig {
                engine: String::from("copilot"),
                model: Some(String::from("gpt-5.4")),
                version: Some(String::from("2026.05")),
                timeout_minutes: Some(30),
            }),
            safe_output_summary: Some(SafeOutputSummary {
                proposed_count: 2,
                executed_count: 1,
                rejected_by_execution_count: 0,
                not_processed_count: 1,
            }),
            safe_output_execution: Some(SafeOutputExecution {
                items: vec![SafeOutputExecutionItem {
                    context: Some(String::from("pr-1")),
                    tool: String::from("create_pull_request"),
                    status: SafeOutputStatus::NotProcessedDueToAggregateGate,
                    proposal: json!({"title": "Fix pipeline", "repository": "repo"}),
                    error: Some(String::from("Batch blocked by detection gate")),
                    result: Some(json!({"status": "blocked"})),
                    rejection_reason: Some(String::from("prompt_injection")),
                    applies_to_whole_batch: true,
                }],
            }),
            rejected_safe_outputs: Some(RejectedSafeOutputsRollup {
                total_rejected: 1,
                by_reason,
                by_threat,
            }),
            detection_analysis: Some(DetectionAnalysis {
                threats: DetectionThreats {
                    prompt_injection: true,
                    secret_leak: false,
                    malicious_patch: false,
                },
                reasons: vec![String::from("Suspicious instruction in fetched content")],
                safe_to_process: false,
                verdict_path: Some(String::from("analyzed_outputs_42\\threat-analysis.json")),
            }),
            mcp_server_health: Some(MCPServerHealth {
                servers: vec![MCPServerStats {
                    name: String::from("github-mcp"),
                    total_calls: 8,
                    error_count: 1,
                    error_rate: 0.125,
                    unreliable: true,
                }],
            }),
            jobs: vec![JobData {
                name: String::from("Agent"),
                status: String::from("completed"),
                result: Some(String::from("succeeded")),
                duration: Some(String::from("4m")),
                started_at: Some(String::from("2026-05-21T12:01:00Z")),
                finished_at: Some(String::from("2026-05-21T12:05:00Z")),
            }],
            downloaded_files: vec![FileInfo {
                path: String::from("logs\\build-42\\agent_outputs_42\\otel.jsonl"),
                size_bytes: 2048,
                sha256: Some(String::from("abc123")),
            }],
            missing_tools: vec![MissingToolReport {
                tool: Some(String::from("azure-devops")),
                context: Some(String::from("work-item-sync")),
                reason: Some(String::from("Tool not configured")),
                timestamp: Some(String::from("2026-05-21T12:03:00Z")),
                extra: json!({"required": true}),
            }],
            missing_data: vec![MissingDataReport {
                tool: Some(String::from("create_work_item")),
                context: Some(String::from("wi-1")),
                reason: Some(String::from("missing title")),
                timestamp: Some(String::from("2026-05-21T12:03:10Z")),
                extra: json!({"field": "title"}),
            }],
            noops: vec![NoopReport {
                tool: Some(String::from("noop")),
                context: Some(String::from("noop-1")),
                reason: Some(String::from("Nothing to do")),
                timestamp: Some(String::from("2026-05-21T12:03:20Z")),
                extra: json!({"kind": "noop"}),
            }],
            mcp_failures: vec![MCPFailureReport {
                tool: Some(String::from("github.search_code")),
                context: Some(String::from("call-17")),
                reason: Some(String::from("HTTP 502")),
                timestamp: Some(String::from("2026-05-21T12:03:30Z")),
                extra: json!({"retryable": true}),
            }],
            firewall_analysis: Some(FirewallAnalysis {
                domains: vec![DomainStat {
                    domain: String::from("api.github.com"),
                    status: String::from("allowed"),
                    request_count: 7,
                    first_seen: Some(String::from("2026-05-21T12:01:10Z")),
                    last_seen: Some(String::from("2026-05-21T12:04:55Z")),
                }],
                total_requests: 10,
                allowed_count: 9,
                denied_count: 1,
            }),
            policy_analysis: Some(PolicyAnalysis {
                policies: vec![PolicyRule {
                    pattern: String::from("https://api.github.com/**"),
                    verdict: String::from("allow"),
                    hit_count: 7,
                }],
                allow_count: 1,
                deny_count: 1,
            }),
            errors: vec![ErrorInfo {
                source: String::from("audit::detection"),
                message: String::from("Threat detection blocked execution"),
                timestamp: Some(String::from("2026-05-21T12:05:00Z")),
            }],
            warnings: vec![ErrorInfo {
                source: String::from("audit::firewall"),
                message: String::from("One request was denied"),
                timestamp: Some(String::from("2026-05-21T12:04:00Z")),
            }],
            tool_usage: vec![ToolUsageInfo {
                name: String::from("edit"),
                call_count: 5,
                total_duration_ms: Some(1500),
            }],
            mcp_tool_usage: Some(MCPToolUsageData {
                tools: vec![MCPToolSummary {
                    name: String::from("github.search_code"),
                    call_count: 3,
                    error_count: 1,
                    max_input_size: 512,
                    max_output_size: 4096,
                }],
            }),
            created_items: vec![CreatedItemReport {
                kind: String::from("pull_request"),
                url: Some(String::from(
                    "https://dev.azure.com/example/project/_git/repo/pullrequest/123",
                )),
                id: Some(String::from("123")),
                title: Some(String::from("Fix pipeline")),
            }],
        }
    }

    #[test]
    fn populated_audit_data_round_trips_through_json() {
        let original = populated_audit_data();
        let json = serde_json::to_string_pretty(&original).expect("serialize populated audit data");
        let round_tripped: AuditData =
            serde_json::from_str(&json).expect("deserialize populated audit data");

        assert_eq!(round_tripped, original);
    }

    #[test]
    fn default_audit_data_round_trips_with_only_required_top_level_keys() {
        let original = AuditData::default();
        let json = serde_json::to_string_pretty(&original).expect("serialize default audit data");
        let round_tripped: AuditData =
            serde_json::from_str(&json).expect("deserialize default audit data");
        let value: Value = serde_json::from_str(&json).expect("parse default audit JSON");
        let keys: Vec<_> = value
            .as_object()
            .expect("top-level JSON object")
            .keys()
            .cloned()
            .collect();

        assert_eq!(round_tripped, original);
        let mut keys_sorted = keys.clone();
        keys_sorted.sort();
        assert_eq!(keys_sorted, vec!["downloaded_files", "metrics", "overview"]);
    }
}
