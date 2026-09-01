//! Tool result infrastructure: traits, macros, and error conversion

use rmcp::ErrorData as McpError;
use rmcp::model::ErrorCode;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use crate::sanitize::{SanitizeConfig, SanitizeContent};
use crate::secure::{GithubTemporaryId, WorkItemTemporaryId};

/// Trait for tool results that include a name field
pub trait ToolResult: Serialize {
    /// The constant name identifier for this tool
    const NAME: &'static str;

    /// Default maximum number of outputs allowed per pipeline run.
    /// Each tool can override this; the operator can further override via `max` in front matter.
    const DEFAULT_MAX: u32 = 1;

    /// Whether this tool performs an external write operation.
    ///
    /// ADO-backed tools receive a write-capable token via
    /// `SYSTEM_ACCESSTOKEN`: by default the pipeline's built-in
    /// `$(System.AccessToken)` (scoped by pipeline settings), or
    /// `$(SC_WRITE_TOKEN)` minted from an ARM service connection when
    /// `permissions.write` is configured. GitHub-backed tools use the separate
    /// Stage 3 GitHub credential.
    ///
    /// This flag is informational — used by audit and (historically) by
    /// the compiler's permission validator. It is NOT a gate. Diagnostic /
    /// read-only tools default to `false`.
    #[allow(dead_code)]
    const REQUIRES_WRITE: bool = false;
}

/// Trait for validating tool parameters before conversion to results.
/// Implement this on your Params struct to add custom validation logic.
/// Uses anyhow::Result so you can use anyhow!, bail!, ensure!, etc.
pub trait Validate {
    /// Validates the parameters, returning an error if invalid.
    fn validate(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

/// A GitHub issue created earlier in the same Stage 3 execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedGithubIssue {
    pub repository: String,
    pub number: u64,
    pub url: String,
}

/// An Azure DevOps work item created earlier in the same Stage 3 execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedWorkItem {
    pub id: u64,
    pub url: String,
}

fn register_resolved_reference<T>(
    registry: &Mutex<HashMap<String, T>>,
    id: String,
    value: T,
    lock_error: &'static str,
) -> anyhow::Result<()> {
    let mut registry = registry
        .lock()
        .map_err(|_| anyhow::anyhow!(lock_error))?;
    if registry.contains_key(&id) {
        anyhow::bail!("temporary_id '{id}' was already used in this run");
    }
    registry.insert(id, value);
    Ok(())
}

/// Context provided to executors during Stage 3 execution
#[derive(Debug, Clone)]
pub struct ExecutionContext {
    /// Azure DevOps organization URL (e.g., `<https://dev.azure.com/myorg>`).
    pub ado_org_url: Option<String>,
    /// Azure DevOps organization name (extracted from ado_org_url, e.g., "myorg")
    pub ado_organization: Option<String>,
    /// Azure DevOps project name
    pub ado_project: Option<String>,
    /// Azure DevOps project GUID (`SYSTEM_TEAMPROJECTID`)
    pub ado_project_id: Option<String>,
    /// Write-capable ADO access token used by Stage 3 executors. Populated
    /// from the `SYSTEM_ACCESSTOKEN` env var, which the compiler maps to
    /// `$(System.AccessToken)` by default or `$(SC_WRITE_TOKEN)` (ARM-minted)
    /// when `permissions.write` is configured.
    pub access_token: Option<String>,
    /// GitHub credential used by GitHub safe outputs in Stage 3.
    pub github_token: Option<String>,
    /// GitHub REST API base URL for Stage 3 issue calls.
    pub github_api_url: String,
    /// Working directory for file operations (safe outputs directory)
    pub working_directory: std::path::PathBuf,
    /// Source checkout directory (BUILD_SOURCESDIRECTORY) where git repos are checked out
    pub source_directory: std::path::PathBuf,
    /// Exact checkout directory for the pipeline's `self` repository.
    ///
    /// In a multi-checkout job `BUILD_SOURCESDIRECTORY` is the common checkout
    /// root, so the compiler passes this path explicitly.
    pub self_repository_directory: std::path::PathBuf,
    /// Per-tool configuration, keyed by tool name
    pub tool_configs: HashMap<String, serde_json::Value>,
    /// Exact `self` repository ID.
    ///
    /// Compiled pipelines no longer project an ID: the compiler resolves the
    /// `self` repository by **name** at compile time, and ADO's REST API
    /// accepts a repository name wherever it accepts an ID. This is populated
    /// only from `ADO_AW_SELF_REPOSITORY_ID`, or from `BUILD_REPOSITORY_ID`
    /// for direct/legacy invocations with no compiler-supplied identity. See
    /// [`ExecutionContext::from_env_lookup`] for why the two sources are never
    /// mixed.
    pub repository_id: Option<String>,
    /// Exact `self` repository name. Compiled pipelines provide
    /// `ADO_AW_SELF_REPOSITORY_NAME`; direct/legacy invocations fall back to
    /// `BUILD_REPOSITORY_NAME`.
    pub repository_name: Option<String>,
    /// Repository provider (from BUILD_REPOSITORY_PROVIDER).
    pub repository_provider: Option<String>,
    /// Allowed repositories for PRs: "self" + checkout list aliases
    /// Maps alias to ADO repo name (e.g., "other-repo" -> "org/other-repo")
    pub allowed_repositories: HashMap<String, String>,
    /// Per-checkout-alias git ref (from `repos: ref`), used to resolve a
    /// per-repo `create-pull-request` target branch when
    /// `infer-target-from-checkout-ref` is set. Maps a checkout alias to its
    /// ref (full `refs/heads/…` or short). `self` is absent (its ref is the
    /// runtime trigger branch, not a static `repos:` ref).
    pub repo_refs: HashMap<String, String>,
    /// Agent execution statistics parsed from OTel JSONL
    pub agent_stats: Option<crate::agent_stats::AgentStats>,
    /// When true, executors validate inputs but skip network calls
    pub dry_run: bool,

    // ── ADO build variables (from BUILD_*/SYSTEM_*) ───────────────────────
    /// Numeric build ID (`BUILD_BUILDID`)
    pub build_id: Option<u64>,
    /// Numeric file-container ID for the current build (`BUILD_CONTAINERID`).
    /// Azure DevOps pre-creates one container per build at job initialization;
    /// all artifacts in the build share this container, differentiated by item path.
    /// Required by `upload-pipeline-artifact` to know where to upload bytes.
    pub build_container_id: Option<u64>,
    /// Orchestration **plan ID** (`SYSTEM_PLANID`) for the current run. Together
    /// with [`Self::timeline_id`] and [`Self::job_id`] this addresses the
    /// current job's timeline record, which is the only target for a
    /// DistributedTask **attachment** create — the API `upload-build-attachment`
    /// uses (the same mechanism as `##vso[task.addattachment]`). Present only
    /// while a job is running; there is no equivalent for an arbitrary build.
    pub plan_id: Option<String>,
    /// Timeline ID (`SYSTEM_TIMELINEID`) for the current run. See
    /// [`Self::plan_id`].
    pub timeline_id: Option<String>,
    /// Timeline **record ID** of the current job (`SYSTEM_JOBID`) — the record a
    /// build attachment is attached to. See [`Self::plan_id`].
    pub job_id: Option<String>,
    /// Human-readable build number (`BUILD_BUILDNUMBER`)
    #[allow(dead_code)]
    pub build_number: Option<String>,
    /// What kicked off this run, e.g. `Manual`, `Schedule`, `ResourceTrigger`,
    /// `PullRequest` (`BUILD_REASON`)
    pub build_reason: Option<String>,
    /// Pipeline definition name (`BUILD_DEFINITIONNAME`)
    #[allow(dead_code)]
    pub definition_name: Option<String>,
    /// Stable numeric pipeline definition identity (`SYSTEM_DEFINITIONID`).
    ///
    /// GitHub comment tools use this value in hidden markers so older comments
    /// can be matched to the originating pipeline without relying on mutable
    /// definition names or agent-authored text.
    #[allow(dead_code)]
    pub definition_id: Option<u64>,
    /// Full source ref, e.g. `refs/heads/main` (`BUILD_SOURCEBRANCH`)
    #[allow(dead_code)]
    pub source_branch: Option<String>,
    /// Short branch name, e.g. `main` (`BUILD_SOURCEBRANCHNAME`)
    #[allow(dead_code)]
    pub source_branch_name: Option<String>,
    /// Source commit SHA (`BUILD_SOURCEVERSION`)
    #[allow(dead_code)]
    pub source_version: Option<String>,

    // ── ResourceTrigger upstream-pipeline variables ───────────────────────
    /// Upstream build ID when triggered by another pipeline
    /// (`BUILD_TRIGGEREDBY_BUILDID`)
    #[allow(dead_code)]
    pub triggered_by_build_id: Option<String>,
    /// Upstream pipeline definition name (`BUILD_TRIGGEREDBY_DEFINITIONNAME`)
    pub triggered_by_definition_name: Option<String>,
    /// Upstream pipeline build number (`BUILD_TRIGGEREDBY_BUILDNUMBER`)
    #[allow(dead_code)]
    pub triggered_by_build_number: Option<String>,
    /// Project hosting the upstream pipeline (`BUILD_TRIGGEREDBY_PROJECTID`)
    #[allow(dead_code)]
    pub triggered_by_project_id: Option<String>,

    // ── PullRequest variables ─────────────────────────────────────────────
    /// PR ID when `BUILD_REASON=PullRequest` (`SYSTEM_PULLREQUEST_PULLREQUESTID`)
    #[allow(dead_code)]
    pub pull_request_id: Option<String>,
    /// PR source branch (`SYSTEM_PULLREQUEST_SOURCEBRANCH`)
    #[allow(dead_code)]
    pub pull_request_source_branch: Option<String>,
    /// PR target branch (`SYSTEM_PULLREQUEST_TARGETBRANCH`)
    #[allow(dead_code)]
    pub pull_request_target_branch: Option<String>,

    /// Per-run dedupe set for `upload-pipeline-artifact` when the
    /// `require-unique-names` config is set. Stores `format!("{}/{}",
    /// effective_build_id, final_name)` keys; the executor checks-and-inserts
    /// before any HTTP call so a second call with the same target build /
    /// artifact name fails fast instead of silently overwriting bytes in
    /// the agent's shared file container.
    ///
    /// Wrapped in `Arc<Mutex<…>>` so all calls in one Stage 3 run see the
    /// same set even though `ExecutionContext` is shared by reference and
    /// the `Clone` semantics need to share state. Each `Default` instance
    /// gets its own fresh empty set, which is correct for tests.
    pub uploaded_pipeline_artifact_keys: Arc<Mutex<HashSet<String>>>,
    /// Temporary GitHub issue IDs resolved by successful `create-github-issue` calls.
    pub resolved_github_issues: Arc<Mutex<HashMap<String, ResolvedGithubIssue>>>,
    /// Temporary work-item IDs resolved by successful `create-work-item` calls.
    pub resolved_work_items: Arc<Mutex<HashMap<String, ResolvedWorkItem>>>,
}

impl ExecutionContext {
    /// Get typed configuration for a specific tool.
    ///
    /// Deserializes the tool's JSON config from front matter and applies
    /// [`SanitizeConfig`] to all textual fields before returning. Missing or
    /// explicit null configs use the tool default; malformed configured JSON
    /// returns an error. The `SanitizeConfig` bound acts as a compile-time
    /// forcing function: adding a new config struct without implementing the
    /// trait won't compile.
    pub fn get_tool_config<T: serde::de::DeserializeOwned + Default + SanitizeConfig>(
        &self,
        tool_name: &str,
    ) -> anyhow::Result<T> {
        let mut config = match self.tool_configs.get(tool_name) {
            None | Some(serde_json::Value::Null) => T::default(),
            Some(value) => {
                let mut value = value.clone();
                // Compiler orchestration metadata, not executor configuration.
                // Both keys are injected into EVERY tool config by Stage 3
                // (`main.rs` for `--source`, `compile/custom_tools.rs` for the
                // `--resolved-config` production path), so a config struct
                // declared `deny_unknown_fields` fails to deserialize unless
                // they are stripped first. Keep this list in sync with every
                // key the compiler injects.
                if let Some(object) = value.as_object_mut() {
                    object.remove("require-approval");
                    object.remove("staged");
                }
                serde_json::from_value(value).map_err(|error| {
                    anyhow::anyhow!("failed to deserialize config for tool '{tool_name}': {error}")
                })?
            }
        };
        config.sanitize_config_fields();
        Ok(config)
    }

    pub fn has_resolved_github_issue(
        &self,
        temporary_id: &GithubTemporaryId,
    ) -> anyhow::Result<bool> {
        let issues = self
            .resolved_github_issues
            .lock()
            .map_err(|_| anyhow::anyhow!("temporary GitHub issue map lock poisoned"))?;
        Ok(issues.contains_key(&temporary_id.canonical()))
    }

    pub fn register_resolved_github_issue(
        &self,
        temporary_id: &GithubTemporaryId,
        issue: ResolvedGithubIssue,
    ) -> anyhow::Result<()> {
        register_resolved_reference(
            &self.resolved_github_issues,
            temporary_id.canonical(),
            issue,
            "temporary GitHub issue map lock poisoned",
        )
    }

    pub fn resolve_github_issue(
        &self,
        temporary_id: &GithubTemporaryId,
    ) -> anyhow::Result<Option<ResolvedGithubIssue>> {
        let issues = self
            .resolved_github_issues
            .lock()
            .map_err(|_| anyhow::anyhow!("temporary GitHub issue map lock poisoned"))?;
        Ok(issues.get(&temporary_id.canonical()).cloned())
    }

    pub fn has_resolved_work_item(
        &self,
        temporary_id: &WorkItemTemporaryId,
    ) -> anyhow::Result<bool> {
        let work_items = self
            .resolved_work_items
            .lock()
            .map_err(|_| anyhow::anyhow!("temporary work-item map lock poisoned"))?;
        Ok(work_items.contains_key(&temporary_id.canonical()))
    }

    pub fn register_resolved_work_item(
        &self,
        temporary_id: &WorkItemTemporaryId,
        work_item: ResolvedWorkItem,
    ) -> anyhow::Result<()> {
        register_resolved_reference(
            &self.resolved_work_items,
            temporary_id.canonical(),
            work_item,
            "temporary work-item map lock poisoned",
        )
    }

    pub fn resolve_work_item(
        &self,
        temporary_id: &WorkItemTemporaryId,
    ) -> anyhow::Result<Option<ResolvedWorkItem>> {
        let work_items = self
            .resolved_work_items
            .lock()
            .map_err(|_| anyhow::anyhow!("temporary work-item map lock poisoned"))?;
        Ok(work_items.get(&temporary_id.canonical()).cloned())
    }
}

/// Extract the organization name from an Azure DevOps org URL.
///
/// Handles both hosted (`https://dev.azure.com/myorg`) and on-prem
/// (`https://server/tfs/myorg`) URLs, with or without a trailing slash.
///
/// Returns `None` if the URL is empty or has no meaningful last segment.
pub fn org_from_url(url: &str) -> Option<String> {
    url.trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

impl ExecutionContext {
    /// Build an `ExecutionContext` from an arbitrary env-var lookup function.
    ///
    /// `Default::default()` calls this with `|k| std::env::var(k).ok()`. Tests
    /// can pass a closure backed by a `HashMap` so they exercise field
    /// population without mutating the (process-global) environment.
    pub fn from_env_lookup<F>(env: F) -> Self
    where
        F: Fn(&str) -> Option<String>,
    {
        // Try AZURE_DEVOPS_ORG_URL first, then fall back to Azure DevOps built-in var
        let ado_org_url =
            env("AZURE_DEVOPS_ORG_URL").or_else(|| env("SYSTEM_TEAMFOUNDATIONCOLLECTIONURI"));

        // Extract organization name from URL (e.g., "https://dev.azure.com/myorg/" -> "myorg")
        let ado_organization = ado_org_url.as_ref().and_then(|url| org_from_url(url));

        // Source directory is where git repos are checked out (BUILD_SOURCESDIRECTORY)
        let source_directory = env("BUILD_SOURCESDIRECTORY")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        let self_repository_directory = env("ADO_AW_SELF_REPOSITORY_DIRECTORY")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| source_directory.clone());

        // Resolve `self`'s identity from exactly one source, never a mix.
        //
        // `BUILD_REPOSITORY_*` describes the repository that *triggered* the
        // run, which is not `checkout: self` on repository-resource-triggered
        // builds (issue #1731). Pairing a compiler-supplied self name with a
        // trigger-scoped ID would be worse than either alone, because
        // consumers that prefer the ID would silently target the wrong
        // repository. So if the compiler supplied either half, the
        // `BUILD_REPOSITORY_*` pair is ignored entirely.
        let compiled_repository_id = env("ADO_AW_SELF_REPOSITORY_ID");
        let compiled_repository_name = env("ADO_AW_SELF_REPOSITORY_NAME");
        let (repository_id, repository_name) =
            if compiled_repository_id.is_some() || compiled_repository_name.is_some() {
                (compiled_repository_id, compiled_repository_name)
            } else {
                (env("BUILD_REPOSITORY_ID"), env("BUILD_REPOSITORY_NAME"))
            };

        Self {
            ado_org_url,
            ado_organization,
            ado_project: env("SYSTEM_TEAMPROJECT"),
            ado_project_id: env("SYSTEM_TEAMPROJECTID"),
            access_token: env("SYSTEM_ACCESSTOKEN").or_else(|| env("AZURE_DEVOPS_EXT_PAT")),
            github_token: env("ADO_AW_GITHUB_TOKEN"),
            github_api_url: env("ADO_AW_GITHUB_API_URL")
                .unwrap_or_else(|| "https://api.github.com".to_string()),
            working_directory: std::env::current_dir().unwrap_or_default(),
            source_directory,
            self_repository_directory,
            tool_configs: HashMap::new(),
            repository_id,
            repository_name,
            repository_provider: env("BUILD_REPOSITORY_PROVIDER"),
            allowed_repositories: HashMap::new(),
            repo_refs: HashMap::new(),
            agent_stats: None,
            dry_run: false,

            // Build identification
            build_id: env("BUILD_BUILDID").and_then(|s| s.parse().ok()),
            build_container_id: env("BUILD_CONTAINERID").and_then(|s| s.parse().ok()),
            plan_id: env("SYSTEM_PLANID"),
            timeline_id: env("SYSTEM_TIMELINEID"),
            job_id: env("SYSTEM_JOBID"),
            build_number: env("BUILD_BUILDNUMBER"),
            build_reason: env("BUILD_REASON"),
            definition_name: env("BUILD_DEFINITIONNAME"),
            definition_id: env("SYSTEM_DEFINITIONID").and_then(|s| s.parse().ok()),
            source_branch: env("BUILD_SOURCEBRANCH"),
            source_branch_name: env("BUILD_SOURCEBRANCHNAME"),
            source_version: env("BUILD_SOURCEVERSION"),

            // ResourceTrigger upstream-pipeline variables
            triggered_by_build_id: env("BUILD_TRIGGEREDBY_BUILDID"),
            triggered_by_definition_name: env("BUILD_TRIGGEREDBY_DEFINITIONNAME"),
            triggered_by_build_number: env("BUILD_TRIGGEREDBY_BUILDNUMBER"),
            triggered_by_project_id: env("BUILD_TRIGGEREDBY_PROJECTID"),

            // Pull request variables
            pull_request_id: env("SYSTEM_PULLREQUEST_PULLREQUESTID"),
            pull_request_source_branch: env("SYSTEM_PULLREQUEST_SOURCEBRANCH"),
            pull_request_target_branch: env("SYSTEM_PULLREQUEST_TARGETBRANCH"),

            // Per-run state for upload-pipeline-artifact dedupe.
            uploaded_pipeline_artifact_keys: Arc::new(Mutex::new(HashSet::new())),
            resolved_github_issues: Arc::new(Mutex::new(HashMap::new())),
            resolved_work_items: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl Default for ExecutionContext {
    fn default() -> Self {
        Self::from_env_lookup(|k| std::env::var(k).ok())
    }
}

/// Result of executing a tool action in Stage 3
#[derive(Debug, Serialize)]
pub struct ExecutionResult {
    /// Whether the execution succeeded
    pub success: bool,
    /// Whether this is a warning (succeeded with issues).
    /// Invariant: warning == true implies success == true.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    warning: bool,
    /// Whether this result represents a budget-exhausted skip.
    /// Invariant: budget_exhausted == true implies success == false.
    /// Set this via [`ExecutionResult::budget_exhausted`] rather than direct
    /// field access so the audit pipeline can key off the structural flag
    /// instead of the human-readable `message`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    budget_exhausted: bool,
    /// Human-readable message describing the outcome
    pub message: String,
    /// Optional additional data (e.g., work item ID)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl ExecutionResult {
    /// Whether this is a warning (succeeded with issues)
    pub fn is_warning(&self) -> bool {
        self.warning
    }

    /// Whether this result represents a budget-exhausted skip.
    ///
    /// The audit pipeline uses this structural flag (rather than parsing the
    /// `message` string) to mark NDJSON manifest records with
    /// `status: "budget_exhausted"`.
    pub fn is_budget_exhausted(&self) -> bool {
        self.budget_exhausted
    }

    /// Create a successful execution result
    pub fn success(message: impl Into<String>) -> Self {
        Self {
            success: true,
            warning: false,
            budget_exhausted: false,
            message: message.into(),
            data: None,
        }
    }

    /// Create a successful execution result with additional data
    pub fn success_with_data(message: impl Into<String>, data: serde_json::Value) -> Self {
        Self {
            success: true,
            warning: false,
            budget_exhausted: false,
            message: message.into(),
            data: Some(data),
        }
    }

    /// Create a warning result (succeeded with issues).
    /// The action completed but something noteworthy occurred.
    /// Exit code 2 signals the pipeline to set SucceededWithIssues.
    pub fn warning(message: impl Into<String>) -> Self {
        Self {
            success: true,
            warning: true,
            budget_exhausted: false,
            message: message.into(),
            data: None,
        }
    }

    /// Create a failed execution result
    pub fn failure(message: impl Into<String>) -> Self {
        Self {
            success: false,
            warning: false,
            budget_exhausted: false,
            message: message.into(),
            data: None,
        }
    }

    /// Create a failed execution result with additional data
    pub fn failure_with_data(message: impl Into<String>, data: serde_json::Value) -> Self {
        Self {
            success: false,
            warning: false,
            budget_exhausted: false,
            message: message.into(),
            data: Some(data),
        }
    }

    /// Create a failed result tagged as a budget-exhausted skip.
    ///
    /// The audit pipeline keys off this structural flag (not the message
    /// string) when emitting `status: "budget_exhausted"` in the executed
    /// NDJSON manifest. Use this instead of [`ExecutionResult::failure`]
    /// whenever a tool entry is skipped because its per-run max has been
    /// reached.
    pub fn budget_exhausted(message: impl Into<String>) -> Self {
        Self {
            success: false,
            warning: false,
            budget_exhausted: true,
            message: message.into(),
            data: None,
        }
    }
}

/// Trait for executing tool results in Stage 3 of the pipeline.
///
/// After the agent generates safe outputs (serialized ToolResult structs),
/// Stage 3 parses these outputs and calls `execute` on each to perform
/// the actual action (e.g., create work items, update files, etc.)
#[async_trait::async_trait]
pub trait Executor: SanitizeContent + Send + Sync {
    /// Internal execution logic. Implementors define this; callers should
    /// use `execute_sanitized()` instead to ensure inputs are sanitized.
    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult>;

    /// Human-readable summary for dry-run output. Override for better messages.
    fn dry_run_summary(&self) -> String {
        "tool execution".to_string()
    }

    /// Sanitize all untrusted fields then execute.
    ///
    /// This is the primary entry point for Stage 3 execution. It guarantees
    /// `sanitize_fields()` is called before `execute_impl()`, making it impossible
    /// to accidentally skip sanitization.
    ///
    /// In dry-run mode, sanitization still runs but `execute_impl()` is skipped —
    /// no network calls are made.
    async fn execute_sanitized(
        &mut self,
        ctx: &ExecutionContext,
    ) -> anyhow::Result<ExecutionResult> {
        self.sanitize_content_fields();
        if ctx.dry_run {
            return Ok(ExecutionResult::success(format!(
                "[DRY-RUN] Would execute: {}",
                self.dry_run_summary()
            )));
        }
        self.execute_impl(ctx).await
    }
}

/// Convert an anyhow error to an MCP error
pub fn anyhow_to_mcp_error(err: anyhow::Error) -> McpError {
    McpError {
        code: ErrorCode::INVALID_PARAMS,
        message: err.to_string().into(),
        data: None,
    }
}

/// Macro to generate a tool result struct with automatic `name` field and `TryFrom<Params>` conversion
///
/// The generated struct derives `Serialize`, `Deserialize`, and `JsonSchema`, making it suitable
/// for both Stage 1 (serialization to safe outputs) and Stage 3 (deserialization for execution).
///
/// # Usage
///
/// Basic (uses trait default of `DEFAULT_MAX = 1`):
/// ```ignore
/// tool_result! {
///     name = "my_tool",
///     params = MyToolParams,
///     pub struct MyToolResult {
///         field1: String,
///         field2: i32,
///     }
/// }
/// ```
///
/// With custom default max (overrides `DEFAULT_MAX` for this tool):
/// ```ignore
/// tool_result! {
///     name = "my_tool",
///     params = MyToolParams,
///     default_max = 5,
///     pub struct MyToolResult {
///         field1: String,
///     }
/// }
/// ```
///
/// Write-requiring tool (sets `REQUIRES_WRITE = true`):
/// ```ignore
/// tool_result! {
///     name = "my_tool",
///     write = true,
///     params = MyToolParams,
///     pub struct MyToolResult {
///         field1: String,
///     }
/// }
/// ```
#[macro_export]
macro_rules! tool_result {
    // write = true, with default_max
    (
        name = $tool_name:literal,
        write = true,
        params = $params:ty,
        default_max = $default_max:literal,
        $(#[$meta:meta])*
        $vis:vis struct $name:ident {
            $(
                $(#[$field_meta:meta])*
                $field_vis:vis $field:ident : $ty:ty
            ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
        $vis struct $name {
            /// Tool identifier
            pub name: String,
            $(
                $(#[$field_meta])*
                pub $field: $ty,
            )*
        }

        impl $crate::safe_outputs::ToolResult for $name {
            const NAME: &'static str = $tool_name;
            const DEFAULT_MAX: u32 = $default_max;
            const REQUIRES_WRITE: bool = true;
        }

        impl TryFrom<$params> for $name {
            type Error = rmcp::ErrorData;

            fn try_from(params: $params) -> Result<Self, Self::Error> {
                <$params as $crate::safe_outputs::Validate>::validate(&params)
                    .map_err($crate::safe_outputs::anyhow_to_mcp_error)?;
                Ok(Self {
                    name: <Self as $crate::safe_outputs::ToolResult>::NAME.to_string(),
                    $($field: params.$field,)*
                })
            }
        }
    };
    // write = true, without default_max
    (
        name = $tool_name:literal,
        write = true,
        params = $params:ty,
        $(#[$meta:meta])*
        $vis:vis struct $name:ident {
            $(
                $(#[$field_meta:meta])*
                $field_vis:vis $field:ident : $ty:ty
            ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
        $vis struct $name {
            /// Tool identifier
            pub name: String,
            $(
                $(#[$field_meta])*
                pub $field: $ty,
            )*
        }

        impl $crate::safe_outputs::ToolResult for $name {
            const NAME: &'static str = $tool_name;
            const REQUIRES_WRITE: bool = true;
        }

        impl TryFrom<$params> for $name {
            type Error = rmcp::ErrorData;

            fn try_from(params: $params) -> Result<Self, Self::Error> {
                <$params as $crate::safe_outputs::Validate>::validate(&params)
                    .map_err($crate::safe_outputs::anyhow_to_mcp_error)?;
                Ok(Self {
                    name: <Self as $crate::safe_outputs::ToolResult>::NAME.to_string(),
                    $($field: params.$field,)*
                })
            }
        }
    };
    // default_max, without write
    (
        name = $tool_name:literal,
        params = $params:ty,
        default_max = $default_max:literal,
        $(#[$meta:meta])*
        $vis:vis struct $name:ident {
            $(
                $(#[$field_meta:meta])*
                $field_vis:vis $field:ident : $ty:ty
            ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
        $vis struct $name {
            /// Tool identifier
            pub name: String,
            $(
                $(#[$field_meta])*
                pub $field: $ty,
            )*
        }

        impl $crate::safe_outputs::ToolResult for $name {
            const NAME: &'static str = $tool_name;
            const DEFAULT_MAX: u32 = $default_max;
        }

        impl TryFrom<$params> for $name {
            type Error = rmcp::ErrorData;

            fn try_from(params: $params) -> Result<Self, Self::Error> {
                <$params as $crate::safe_outputs::Validate>::validate(&params)
                    .map_err($crate::safe_outputs::anyhow_to_mcp_error)?;
                Ok(Self {
                    name: <Self as $crate::safe_outputs::ToolResult>::NAME.to_string(),
                    $($field: params.$field,)*
                })
            }
        }
    };
    // basic (no write, no default_max)
    (
        name = $tool_name:literal,
        params = $params:ty,
        $(#[$meta:meta])*
        $vis:vis struct $name:ident {
            $(
                $(#[$field_meta:meta])*
                $field_vis:vis $field:ident : $ty:ty
            ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
        $vis struct $name {
            /// Tool identifier
            pub name: String,
            $(
                $(#[$field_meta])*
                pub $field: $ty,
            )*
        }

        impl $crate::safe_outputs::ToolResult for $name {
            const NAME: &'static str = $tool_name;
        }

        impl TryFrom<$params> for $name {
            type Error = rmcp::ErrorData;

            fn try_from(params: $params) -> Result<Self, Self::Error> {
                <$params as $crate::safe_outputs::Validate>::validate(&params)
                    .map_err($crate::safe_outputs::anyhow_to_mcp_error)?;
                Ok(Self {
                    name: <Self as $crate::safe_outputs::ToolResult>::NAME.to_string(),
                    $($field: params.$field,)*
                })
            }
        }
    };
}

/// Derive a `&[&str]` array of tool names from a list of types implementing `ToolResult`.
///
/// This macro is the foundation for compile-time safe output tool list generation.
/// Instead of maintaining string arrays by hand, list the concrete types and the
/// macro extracts each type's `NAME` constant automatically.
///
/// # Usage
/// ```ignore
/// const MY_TOOLS: &[&str] = tool_names![FooResult, BarResult];
/// // expands to: &[FooResult::NAME, BarResult::NAME]
/// ```
#[macro_export]
macro_rules! tool_names {
    ($($ty:ty),* $(,)?) => {
        &[$(<$ty as $crate::safe_outputs::ToolResult>::NAME),*]
    };
}

/// Derive `ALL_KNOWN_SAFE_OUTPUTS` from a list of types plus extra string literals.
///
/// All tool types go before the semicolon; non-MCP string keys go after it.
///
/// # Usage
/// ```ignore
/// const ALL: &[&str] = all_safe_output_names![
///     WriteToolA, WriteToolB,   // write-requiring types
///     DiagToolA, DiagToolB;     // diagnostic types (all types before `;`)
///     "memory"                  // non-MCP string keys (after `;`)
/// ];
/// ```
#[macro_export]
macro_rules! all_safe_output_names {
    ($($ty:ty),* $(,)?; $($extra:expr),* $(,)?) => {
        &[$(<$ty as $crate::safe_outputs::ToolResult>::NAME),*, $($extra),*]
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_execution_result_success() {
        let r = ExecutionResult::success("all good");
        assert!(r.success);
        assert_eq!(r.message, "all good");
        assert!(r.data.is_none());
    }

    #[test]
    fn test_execution_result_success_with_data() {
        let data = serde_json::json!({"id": 42});
        let r = ExecutionResult::success_with_data("created", data.clone());
        assert!(r.success);
        assert_eq!(r.message, "created");
        assert_eq!(r.data, Some(data));
    }

    #[test]
    fn test_execution_result_failure() {
        let r = ExecutionResult::failure("something broke");
        assert!(!r.success);
        assert_eq!(r.message, "something broke");
        assert!(r.data.is_none());
    }

    #[test]
    fn test_anyhow_to_mcp_error_preserves_message() {
        let err = anyhow::anyhow!("test error message");
        let mcp_err = anyhow_to_mcp_error(err);
        assert_eq!(mcp_err.message, "test error message");
    }

    #[test]
    fn test_anyhow_to_mcp_error_uses_invalid_params_code() {
        let err = anyhow::anyhow!("some error");
        let mcp_err = anyhow_to_mcp_error(err);
        assert_eq!(mcp_err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    }

    // ── ExecutionResult::warning / is_warning tests ───────────────────────

    #[test]
    fn test_execution_result_warning_sets_success_and_warning() {
        let r = ExecutionResult::warning("PR created but auto-complete failed");
        assert!(r.success, "warning result should have success=true");
        assert!(r.is_warning(), "warning result should have warning=true");
        assert_eq!(r.message, "PR created but auto-complete failed");
        assert!(r.data.is_none());
    }

    #[test]
    fn test_execution_result_success_is_not_warning() {
        let r = ExecutionResult::success("all good");
        assert!(!r.is_warning(), "success result should not be a warning");
    }

    #[test]
    fn test_execution_result_failure_is_not_warning() {
        let r = ExecutionResult::failure("something broke");
        assert!(!r.is_warning(), "failure result should not be a warning");
    }

    // ── ExecutionResult::budget_exhausted / is_budget_exhausted tests ─────

    #[test]
    fn test_execution_result_budget_exhausted_sets_flag_and_failure() {
        let r = ExecutionResult::budget_exhausted(
            "Skipped (work item #42): maximum create-work-item count (3) already reached.",
        );
        assert!(
            !r.success,
            "budget_exhausted result should have success=false"
        );
        assert!(!r.is_warning(), "budget_exhausted should not be a warning");
        assert!(
            r.is_budget_exhausted(),
            "budget_exhausted result should report budget_exhausted=true"
        );
        assert!(r.data.is_none());
    }

    #[test]
    fn test_execution_result_failure_is_not_budget_exhausted() {
        let r = ExecutionResult::failure("permission denied");
        assert!(
            !r.is_budget_exhausted(),
            "ordinary failure should not be flagged as budget-exhausted"
        );
    }

    #[test]
    fn test_execution_result_success_is_not_budget_exhausted() {
        let r = ExecutionResult::success("done");
        assert!(
            !r.is_budget_exhausted(),
            "success result should not be flagged as budget-exhausted"
        );
    }

    #[test]
    fn test_execution_result_budget_exhausted_serializes_flag() {
        let r =
            ExecutionResult::budget_exhausted("Skipped: maximum noop count (1) already reached");
        let json = serde_json::to_value(&r).expect("serialize");
        assert_eq!(
            json.get("budget_exhausted").and_then(|v| v.as_bool()),
            Some(true),
            "budget_exhausted=true should be serialized; got: {json}"
        );
    }

    #[test]
    fn test_execution_result_failure_omits_budget_exhausted() {
        let r = ExecutionResult::failure("permission denied");
        let json = serde_json::to_value(&r).expect("serialize");
        assert!(
            json.get("budget_exhausted").is_none(),
            "budget_exhausted=false should be omitted from JSON; got: {json}"
        );
    }

    // ── ExecutionContext::get_tool_config sanitization tests ──────────────

    /// Test config struct used to verify that `get_tool_config` applies
    /// `sanitize_config_fields()` before returning the deserialized value.
    #[derive(Default, serde::Deserialize)]
    struct TestConfigForSanitization {
        value: String,
    }

    impl crate::sanitize::SanitizeConfig for TestConfigForSanitization {
        fn sanitize_config_fields(&mut self) {
            self.value = crate::sanitize::sanitize_config(&self.value);
        }
    }

    #[test]
    fn test_get_tool_config_sanitizes_vso_pipeline_command() {
        let mut ctx = ExecutionContext::default();
        ctx.tool_configs.insert(
            "my-tool".to_string(),
            serde_json::json!({ "value": "##vso[task.setvariable variable=secret]injected" }),
        );
        let config: TestConfigForSanitization =
            ctx.get_tool_config("my-tool").expect("config should parse");
        assert!(
            !config.value.contains("##vso[task."),
            "Injected ##vso[ command should be neutralized; got: {}",
            config.value
        );
        assert!(
            config.value.contains("`##vso[`"),
            "Pipeline command should be wrapped in backticks; got: {}",
            config.value
        );
    }

    #[test]
    fn test_get_tool_config_missing_and_null_use_defaults() {
        let missing: TestConfigForSanitization = ExecutionContext::default()
            .get_tool_config("missing-tool")
            .expect("missing config should use defaults");
        assert!(missing.value.is_empty());

        let mut ctx = ExecutionContext::default();
        ctx.tool_configs
            .insert("null-tool".to_string(), serde_json::Value::Null);
        let null: TestConfigForSanitization = ctx
            .get_tool_config("null-tool")
            .expect("null config should use defaults");
        assert!(null.value.is_empty());
    }

    #[test]
    fn test_get_tool_config_rejects_malformed_github_config() {
        let mut ctx = ExecutionContext::default();
        ctx.tool_configs.insert(
            "create-github-issue".to_string(),
            serde_json::json!({ "allowed-labels": "not-an-array" }),
        );

        let error = ctx
            .get_tool_config::<crate::safe_outputs::CreateGithubIssueConfig>("create-github-issue")
            .expect_err("malformed GitHub config must fail closed");
        assert!(
            error
                .to_string()
                .contains("failed to deserialize config for tool 'create-github-issue'"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn test_get_tool_config_rejects_malformed_non_github_config() {
        let mut ctx = ExecutionContext::default();
        ctx.tool_configs.insert(
            "add-build-tag".to_string(),
            serde_json::json!({ "allow-any-build": "not-a-boolean" }),
        );

        let error = ctx
            .get_tool_config::<crate::safe_outputs::AddBuildTagConfig>("add-build-tag")
            .expect_err("malformed non-GitHub config must fail closed");
        assert!(
            error
                .to_string()
                .contains("failed to deserialize config for tool 'add-build-tag'"),
            "unexpected error: {error}"
        );
    }

    // ── ADO build variable capture tests (use from_env_lookup so they
    //    don't mutate the process-global environment) ─────────────────────

    fn env_from(map: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let owned: HashMap<String, String> = map
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| owned.get(k).cloned()
    }

    #[test]
    fn test_from_env_lookup_populates_build_fields() {
        let ctx = ExecutionContext::from_env_lookup(env_from(&[
            ("BUILD_BUILDID", "12345"),
            ("BUILD_BUILDNUMBER", "20240101.1"),
            ("BUILD_REASON", "Manual"),
            ("BUILD_DEFINITIONNAME", "My Pipeline"),
            ("SYSTEM_DEFINITIONID", "987"),
            ("BUILD_SOURCEBRANCH", "refs/heads/main"),
            ("BUILD_SOURCEBRANCHNAME", "main"),
            ("BUILD_SOURCEVERSION", "abc1234"),
        ]));
        assert_eq!(ctx.build_id, Some(12345));
        assert_eq!(ctx.build_number.as_deref(), Some("20240101.1"));
        assert_eq!(ctx.build_reason.as_deref(), Some("Manual"));
        assert_eq!(ctx.definition_name.as_deref(), Some("My Pipeline"));
        assert_eq!(ctx.definition_id, Some(987));
        assert_eq!(ctx.source_branch.as_deref(), Some("refs/heads/main"));
        assert_eq!(ctx.source_branch_name.as_deref(), Some("main"));
        assert_eq!(ctx.source_version.as_deref(), Some("abc1234"));
    }

    #[test]
    fn test_from_env_lookup_populates_checkout_directories() {
        let ctx = ExecutionContext::from_env_lookup(env_from(&[
            ("BUILD_SOURCESDIRECTORY", "C:\\agent\\s"),
            ("ADO_AW_SELF_REPOSITORY_DIRECTORY", "C:\\agent\\s\\ado-aw"),
        ]));

        assert_eq!(
            ctx.source_directory,
            std::path::PathBuf::from("C:\\agent\\s")
        );
        assert_eq!(
            ctx.self_repository_directory,
            std::path::PathBuf::from("C:\\agent\\s\\ado-aw")
        );
    }

    #[test]
    fn test_from_env_lookup_self_directory_falls_back_to_source_directory() {
        let ctx = ExecutionContext::from_env_lookup(env_from(&[(
            "BUILD_SOURCESDIRECTORY",
            "C:\\agent\\s",
        )]));

        assert_eq!(ctx.self_repository_directory, ctx.source_directory);
    }

    #[test]
    fn test_from_env_lookup_prefers_compiler_owned_self_identity() {
        // Both halves supplied explicitly (manual/testing): use them as a pair.
        let ctx = ExecutionContext::from_env_lookup(env_from(&[
            ("ADO_AW_SELF_REPOSITORY_ID", "self-id"),
            ("ADO_AW_SELF_REPOSITORY_NAME", "project/self-repo"),
            ("BUILD_REPOSITORY_ID", "trigger-id"),
            ("BUILD_REPOSITORY_NAME", "project/trigger-repo"),
        ]));

        assert_eq!(ctx.repository_id.as_deref(), Some("self-id"));
        assert_eq!(ctx.repository_name.as_deref(), Some("project/self-repo"));
    }

    #[test]
    fn test_from_env_lookup_name_only_self_identity_ignores_trigger_id() {
        // The shape compiled pipelines emit: a compile-time self name and no
        // ID. The trigger-scoped BUILD_REPOSITORY_ID must NOT be paired with
        // it, or consumers preferring the ID would target the wrong repo.
        let ctx = ExecutionContext::from_env_lookup(env_from(&[
            ("ADO_AW_SELF_REPOSITORY_NAME", "self-repo"),
            ("BUILD_REPOSITORY_ID", "trigger-id"),
            ("BUILD_REPOSITORY_NAME", "trigger-repo"),
        ]));

        assert_eq!(ctx.repository_name.as_deref(), Some("self-repo"));
        assert_eq!(ctx.repository_id, None);
    }

    #[test]
    fn test_from_env_lookup_self_identity_falls_back_to_build_variables() {
        let ctx = ExecutionContext::from_env_lookup(env_from(&[
            ("BUILD_REPOSITORY_ID", "build-id"),
            ("BUILD_REPOSITORY_NAME", "project/build-repo"),
        ]));

        assert_eq!(ctx.repository_id.as_deref(), Some("build-id"));
        assert_eq!(ctx.repository_name.as_deref(), Some("project/build-repo"));
    }

    #[test]
    fn test_from_env_lookup_build_id_none_for_non_numeric() {
        let ctx = ExecutionContext::from_env_lookup(env_from(&[("BUILD_BUILDID", "not-a-number")]));
        assert!(ctx.build_id.is_none());
    }

    #[test]
    fn test_from_env_lookup_build_id_none_when_unset() {
        let ctx = ExecutionContext::from_env_lookup(env_from(&[]));
        assert!(ctx.build_id.is_none());
    }

    #[test]
    fn test_from_env_lookup_definition_id_none_for_invalid_or_unset() {
        let invalid =
            ExecutionContext::from_env_lookup(env_from(&[("SYSTEM_DEFINITIONID", "invalid")]));
        assert!(invalid.definition_id.is_none());
        assert!(
            ExecutionContext::from_env_lookup(env_from(&[]))
                .definition_id
                .is_none()
        );
    }

    #[test]
    fn test_from_env_lookup_build_container_id_parses_numeric() {
        let ctx = ExecutionContext::from_env_lookup(env_from(&[("BUILD_CONTAINERID", "112233")]));
        assert_eq!(ctx.build_container_id, Some(112233));
    }

    #[test]
    fn test_from_env_lookup_build_container_id_none_for_non_numeric() {
        let ctx =
            ExecutionContext::from_env_lookup(env_from(&[("BUILD_CONTAINERID", "not-numeric")]));
        assert!(ctx.build_container_id.is_none());
    }

    #[test]
    fn test_from_env_lookup_build_container_id_none_when_unset() {
        let ctx = ExecutionContext::from_env_lookup(env_from(&[]));
        assert!(ctx.build_container_id.is_none());
    }

    #[test]
    fn test_from_env_lookup_timeline_coordinates() {
        // SYSTEM_PLANID / SYSTEM_TIMELINEID / SYSTEM_JOBID address the current
        // job's timeline record — the target for a build attachment.
        let ctx = ExecutionContext::from_env_lookup(env_from(&[
            ("SYSTEM_PLANID", "11111111-1111-1111-1111-111111111111"),
            ("SYSTEM_TIMELINEID", "22222222-2222-2222-2222-222222222222"),
            ("SYSTEM_JOBID", "33333333-3333-3333-3333-333333333333"),
        ]));
        assert_eq!(
            ctx.plan_id.as_deref(),
            Some("11111111-1111-1111-1111-111111111111")
        );
        assert_eq!(
            ctx.timeline_id.as_deref(),
            Some("22222222-2222-2222-2222-222222222222")
        );
        assert_eq!(
            ctx.job_id.as_deref(),
            Some("33333333-3333-3333-3333-333333333333")
        );
    }

    #[test]
    fn test_from_env_lookup_timeline_coordinates_none_when_unset() {
        let ctx = ExecutionContext::from_env_lookup(env_from(&[]));
        assert!(ctx.plan_id.is_none());
        assert!(ctx.timeline_id.is_none());
        assert!(ctx.job_id.is_none());
    }

    #[test]
    fn test_from_env_lookup_populates_triggered_by_fields() {
        let ctx = ExecutionContext::from_env_lookup(env_from(&[
            ("BUILD_REASON", "ResourceTrigger"),
            ("BUILD_TRIGGEREDBY_BUILDID", "42"),
            ("BUILD_TRIGGEREDBY_DEFINITIONNAME", "Upstream Build"),
            ("BUILD_TRIGGEREDBY_BUILDNUMBER", "20240101.7"),
            ("BUILD_TRIGGEREDBY_PROJECTID", "proj-guid"),
        ]));
        assert_eq!(ctx.build_reason.as_deref(), Some("ResourceTrigger"));
        assert_eq!(ctx.triggered_by_build_id.as_deref(), Some("42"));
        assert_eq!(
            ctx.triggered_by_definition_name.as_deref(),
            Some("Upstream Build")
        );
        assert_eq!(ctx.triggered_by_build_number.as_deref(), Some("20240101.7"));
        assert_eq!(ctx.triggered_by_project_id.as_deref(), Some("proj-guid"));
    }

    #[test]
    fn test_from_env_lookup_triggered_by_none_when_unset() {
        let ctx = ExecutionContext::from_env_lookup(env_from(&[]));
        assert!(ctx.triggered_by_build_id.is_none());
        assert!(ctx.triggered_by_definition_name.is_none());
        assert!(ctx.triggered_by_build_number.is_none());
        assert!(ctx.triggered_by_project_id.is_none());
    }

    #[test]
    fn test_from_env_lookup_populates_pull_request_fields() {
        let ctx = ExecutionContext::from_env_lookup(env_from(&[
            ("BUILD_REASON", "PullRequest"),
            ("SYSTEM_PULLREQUEST_PULLREQUESTID", "789"),
            ("SYSTEM_PULLREQUEST_SOURCEBRANCH", "refs/heads/feature"),
            ("SYSTEM_PULLREQUEST_TARGETBRANCH", "refs/heads/main"),
        ]));
        assert_eq!(ctx.pull_request_id.as_deref(), Some("789"));
        assert_eq!(
            ctx.pull_request_source_branch.as_deref(),
            Some("refs/heads/feature")
        );
        assert_eq!(
            ctx.pull_request_target_branch.as_deref(),
            Some("refs/heads/main")
        );
    }

    #[test]
    fn test_from_env_lookup_pull_request_none_when_unset() {
        let ctx = ExecutionContext::from_env_lookup(env_from(&[]));
        assert!(ctx.pull_request_id.is_none());
        assert!(ctx.pull_request_source_branch.is_none());
        assert!(ctx.pull_request_target_branch.is_none());
    }

    /// Build a context whose tool config carries the orchestration keys the
    /// compiler injects into EVERY tool config.
    fn ctx_with_injected_keys(tool: &str, mut config: serde_json::Value) -> ExecutionContext {
        let object = config.as_object_mut().expect("config must be an object");
        // Mirrors `main.rs` (--source) and `compile/custom_tools.rs`
        // (--resolved-config, the production path).
        object.insert("staged".to_string(), serde_json::Value::Bool(false));
        object.insert(
            "require-approval".to_string(),
            serde_json::Value::Bool(false),
        );
        let mut tool_configs = HashMap::new();
        tool_configs.insert(tool.to_string(), config);
        ExecutionContext {
            tool_configs,
            ..Default::default()
        }
    }

    /// Regression guard for compiler-only orchestration keys.
    ///
    /// `CreateGithubIssueConfig` and `SetGithubIssueTypeConfig` are declared
    /// `#[serde(deny_unknown_fields)]`, so the compiler-injected `staged` /
    /// `require-approval` keys must be stripped before strict deserialization.
    #[test]
    fn test_get_tool_config_survives_compiler_injected_orchestration_keys() {
        let ctx = ctx_with_injected_keys(
            "create-github-issue",
            serde_json::json!({
                "target-repo": "octo/scratch",
                "title-prefix": "[prefix] ",
                "labels": ["static-label"],
                "allowed-labels": ["agent-*"],
                "require-temporary-id": true,
                "max": 3,
            }),
        );
        let config: crate::safe_outputs::CreateGithubIssueConfig = ctx
            .get_tool_config("create-github-issue")
            .expect("compiler-only keys should be stripped");
        assert_eq!(
            config.target_repo.as_deref(),
            Some("octo/scratch"),
            "operator target-repo must survive the injected orchestration keys"
        );
        assert_eq!(config.title_prefix.as_deref(), Some("[prefix] "));
        assert_eq!(config.labels, vec!["static-label".to_string()]);
        assert_eq!(config.allowed_labels, vec!["agent-*".to_string()]);
        assert!(config.require_temporary_id);
        assert_eq!(config.max, Some(3));
    }

    #[test]
    fn test_get_tool_config_survives_injected_keys_for_set_github_issue_type() {
        let ctx = ctx_with_injected_keys(
            "set-github-issue-type",
            serde_json::json!({ "target-repo": "octo/scratch", "allowed": ["Bug"] }),
        );
        let config: crate::safe_outputs::SetGithubIssueTypeConfig = ctx
            .get_tool_config("set-github-issue-type")
            .expect("compiler-only keys should be stripped");
        assert_eq!(config.target_repo.as_deref(), Some("octo/scratch"));
        // An empty `allowed` list is default-ALLOW, so a silent wipe here fails
        // open — any issue type would be accepted.
        assert_eq!(config.allowed, vec!["Bug".to_string()]);
    }
}
