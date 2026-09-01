//! `assign-github-issue-milestone` safe output.

use anyhow::ensure;
use log::{debug, info};
use reqwest::Method;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::safe_outputs::{
    ExecutionContext, ExecutionResult, Executor, GithubClient, GithubIssueNumber,
    GithubMutationFilters, GithubRepositoryPolicy, GithubTargetCapabilities, Validate,
    resolve_github_issue_target, validate_github_mutation_filter_config,
    validate_github_mutation_filters, validate_github_repository,
    validate_github_target_capability,
};
use crate::sanitize::{SanitizeConfig, SanitizeContent, sanitize_config};
use crate::tool_result;
use crate::validate::reject_pipeline_injection;

const MAX_MILESTONE_TITLE_LEN: usize = 256;

#[derive(Deserialize, JsonSchema)]
pub struct AssignGithubIssueMilestoneParams {
    /// Positive GitHub issue number or a temporary ID from create-github-issue.
    pub issue_number: GithubIssueNumber,
    /// Existing milestone number.
    #[serde(default)]
    pub milestone_number: Option<u64>,
    /// Existing milestone title, or a new title when `auto-create` is enabled.
    #[serde(default)]
    pub milestone_title: Option<String>,
    /// Optional target repository. Must exactly match `target-repo` or an
    /// `allowed-repos` entry.
    #[serde(default)]
    pub repository: Option<String>,
}

impl Validate for AssignGithubIssueMilestoneParams {
    fn validate(&self) -> anyhow::Result<()> {
        self.issue_number.validate("issue_number")?;
        ensure!(
            self.milestone_number.is_some() ^ self.milestone_title.is_some(),
            "exactly one of milestone_number or milestone_title must be provided"
        );
        if let Some(number) = self.milestone_number {
            ensure!(number > 0, "milestone_number must be positive");
        }
        if let Some(title) = self.milestone_title.as_deref() {
            validate_milestone_title(title, "milestone_title")?;
        }
        if let Some(repository) = self.repository.as_deref() {
            validate_github_repository(repository)?;
        }
        Ok(())
    }
}

tool_result! {
    name = "assign-github-issue-milestone",
    write = true,
    params = AssignGithubIssueMilestoneParams,
    default_max = 1,
    /// Result of assigning a milestone to a GitHub issue.
    pub struct AssignGithubIssueMilestoneResult {
        issue_number: GithubIssueNumber,
        #[serde(default)]
        milestone_number: Option<u64>,
        #[serde(default)]
        milestone_title: Option<String>,
        #[serde(default)]
        repository: Option<String>,
    }
}

impl SanitizeContent for AssignGithubIssueMilestoneResult {
    fn sanitize_content_fields(&mut self) {
        self.milestone_title = self.milestone_title.as_deref().map(sanitize_config);
        self.repository = self.repository.as_deref().map(sanitize_config);
    }
}

/// A milestone title or numeric milestone ID accepted by operator policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum GithubMilestoneAllowance {
    Number(u64),
    Title(String),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssignGithubIssueMilestoneConfig {
    #[serde(default, rename = "target-repo")]
    pub target_repo: Option<String>,
    #[serde(default, rename = "allowed-repos")]
    pub allowed_repos: Vec<String>,
    #[serde(default, rename = "required-labels")]
    pub required_labels: Vec<String>,
    #[serde(default, rename = "required-title-prefix")]
    pub required_title_prefix: Option<String>,
    /// Allowed milestone titles or numeric milestone IDs. Empty permits any
    /// existing milestone; auto-created milestones still use their requested title.
    #[serde(default)]
    pub allowed: Vec<GithubMilestoneAllowance>,
    #[serde(default, rename = "auto-create")]
    pub auto_create: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<u32>,
}

impl SanitizeConfig for AssignGithubIssueMilestoneConfig {
    fn sanitize_config_fields(&mut self) {
        self.target_repo = self.target_repo.as_deref().map(sanitize_config);
        self.allowed_repos = self
            .allowed_repos
            .iter()
            .map(|value| sanitize_config(value))
            .collect();
        self.required_labels = self
            .required_labels
            .iter()
            .map(|value| sanitize_config(value))
            .collect();
        self.required_title_prefix = self.required_title_prefix.as_deref().map(sanitize_config);
        for allowance in &mut self.allowed {
            if let GithubMilestoneAllowance::Title(title) = allowance {
                *title = sanitize_config(title);
            }
        }
    }
}

fn validate_milestone_title(title: &str, field: &str) -> anyhow::Result<()> {
    ensure!(!title.trim().is_empty(), "{field} must not be empty");
    ensure!(
        title.len() <= MAX_MILESTONE_TITLE_LEN,
        "{field} must be {MAX_MILESTONE_TITLE_LEN} characters or fewer"
    );
    reject_pipeline_injection(title, field)
}

pub(crate) fn validate_assign_github_issue_milestone_config(
    config: &AssignGithubIssueMilestoneConfig,
) -> anyhow::Result<()> {
    validate_github_mutation_filter_config(GithubMutationFilters {
        required_labels: &config.required_labels,
        required_title_prefix: config.required_title_prefix.as_deref(),
    })?;
    for allowance in &config.allowed {
        match allowance {
            GithubMilestoneAllowance::Number(number) => {
                ensure!(*number > 0, "allowed milestone numbers must be positive");
            }
            GithubMilestoneAllowance::Title(title) => {
                validate_milestone_title(title, "allowed milestone title")?;
            }
        }
    }
    Ok(())
}

fn milestone_is_allowed(
    config: &AssignGithubIssueMilestoneConfig,
    number: Option<u64>,
    title: &str,
) -> bool {
    config.allowed.is_empty()
        || config.allowed.iter().any(|allowance| match allowance {
            GithubMilestoneAllowance::Number(allowed) => number == Some(*allowed),
            GithubMilestoneAllowance::Title(allowed) => allowed == title,
        })
}

#[async_trait::async_trait]
impl Executor for AssignGithubIssueMilestoneResult {
    fn dry_run_summary(&self) -> String {
        let target = self.issue_number.to_string();
        let milestone = match (self.milestone_number, self.milestone_title.as_deref()) {
            (Some(number), _) => format!("milestone #{number}"),
            (_, Some(title)) => format!("milestone '{title}'"),
            _ => "an invalid milestone".to_string(),
        };
        format!("assign {milestone} to GitHub issue {target}")
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        const TOOL: &str = "assign-github-issue-milestone";
        if !ctx.tool_configs.contains_key(TOOL) {
            return Ok(ExecutionResult::failure(format!(
                "{TOOL} is not configured for this workflow"
            )));
        }
        let token = match ctx.github_token.as_ref() {
            Some(token) => token,
            None => {
                return Ok(ExecutionResult::failure(
                    "ADO_AW_GITHUB_TOKEN is not set; configure safe-outputs.github-token \
                     or safe-outputs.github-app",
                ));
            }
        };
        let config: AssignGithubIssueMilestoneConfig = ctx.get_tool_config(TOOL)?;
        validate_assign_github_issue_milestone_config(&config)?;

        let target = match resolve_github_issue_target(
            &self.issue_number,
            self.repository.as_deref(),
            GithubRepositoryPolicy::new(config.target_repo.as_deref(), &config.allowed_repos),
            ctx,
        )? {
            Ok(target) => target,
            Err(result) => return Ok(result),
        };
        let client = GithubClient::new(&ctx.github_api_url, token)?;
        let filters = GithubMutationFilters {
            required_labels: &config.required_labels,
            required_title_prefix: config.required_title_prefix.as_deref(),
        };

        // Fetch and validate the target before milestone creation or assignment.
        let metadata = match client.get_issue(&target.repository, target.number).await? {
            Ok(metadata) => metadata,
            Err(error) => return Ok(ExecutionResult::failure(error.to_string())),
        };
        if let Err(result) =
            validate_github_target_capability(&metadata, GithubTargetCapabilities::ISSUES_ONLY)
        {
            return Ok(result);
        }
        if let Err(result) = validate_github_mutation_filters(&metadata, filters) {
            return Ok(result);
        }

        // Resolve against every page before the first write. This also gives
        // numeric requests their canonical title for policy evaluation.
        let milestones = match client.list_milestones(&target.repository).await? {
            Ok(milestones) => milestones,
            Err(error) => return Ok(ExecutionResult::failure(error.to_string())),
        };

        let (milestone_number, milestone_title, created) = if let Some(requested_number) =
            self.milestone_number
        {
            let Some(milestone) = milestones
                .iter()
                .find(|milestone| milestone.number == requested_number)
            else {
                return Ok(ExecutionResult::failure(format!(
                    "GitHub milestone #{requested_number} does not exist in {}",
                    target.repository
                )));
            };
            if !milestone_is_allowed(&config, Some(milestone.number), &milestone.title) {
                return Ok(ExecutionResult::failure(format!(
                    "GitHub milestone #{} ('{}') is not in the allowed list",
                    milestone.number,
                    crate::sanitize::neutralize_pipeline_commands(&milestone.title)
                )));
            }
            (milestone.number, milestone.title.clone(), false)
        } else {
            let requested_title = self
                .milestone_title
                .as_deref()
                .expect("validated result has a milestone title");
            let matches: Vec<_> = milestones
                .iter()
                .filter(|milestone| milestone.title == requested_title)
                .collect();
            if matches.len() > 1 {
                return Ok(ExecutionResult::failure(format!(
                    "multiple GitHub milestones in {} have the exact title '{}'; use \
                         milestone_number to disambiguate",
                    target.repository,
                    crate::sanitize::neutralize_pipeline_commands(requested_title)
                )));
            }
            if let Some(milestone) = matches.first() {
                if !milestone_is_allowed(&config, Some(milestone.number), &milestone.title) {
                    return Ok(ExecutionResult::failure(format!(
                        "GitHub milestone '{}' (#{}) is not in the allowed list",
                        crate::sanitize::neutralize_pipeline_commands(&milestone.title),
                        milestone.number
                    )));
                }
                (milestone.number, milestone.title.clone(), false)
            } else {
                if !milestone_is_allowed(&config, None, requested_title) {
                    return Ok(ExecutionResult::failure(format!(
                        "GitHub milestone '{}' is not in the allowed list",
                        crate::sanitize::neutralize_pipeline_commands(requested_title)
                    )));
                }
                if !config.auto_create {
                    return Ok(ExecutionResult::failure(format!(
                        "GitHub milestone '{}' does not exist in {} and auto-create is false",
                        crate::sanitize::neutralize_pipeline_commands(requested_title),
                        target.repository
                    )));
                }
                let response = client
                    .send(
                        Method::POST,
                        client.milestones_url(&target.repository)?,
                        Some(&serde_json::json!({ "title": requested_title })),
                    )
                    .await?;
                let response = match response.require_success("Failed to create GitHub milestone") {
                    Ok(response) => response,
                    Err(error) => {
                        return Ok(ExecutionResult::failure(error.to_string()));
                    }
                };
                let payload: serde_json::Value =
                    response.json("Failed to parse created GitHub milestone")?;
                let Some(number) = payload
                    .get("number")
                    .and_then(serde_json::Value::as_u64)
                    .filter(|number| *number > 0)
                else {
                    return Ok(ExecutionResult::failure(
                        "GitHub create milestone response contained no positive milestone number",
                    ));
                };
                (number, requested_title.to_string(), true)
            }
        };

        let issue_url = client.issue_url(&target.repository, target.number)?;
        debug!(
            "Assigning GitHub milestone #{} to {}#{}",
            milestone_number, target.repository, target.number
        );
        let response = client
            .send(
                Method::PATCH,
                issue_url,
                Some(&serde_json::json!({ "milestone": milestone_number })),
            )
            .await?;
        if let Err(error) = response.require_success("Failed to assign GitHub milestone") {
            return Ok(ExecutionResult::failure(error.to_string()));
        }

        info!(
            "Assigned GitHub milestone #{} to {}#{}",
            milestone_number, target.repository, target.number
        );
        Ok(ExecutionResult::success_with_data(
            format!(
                "Assigned milestone '{}' (#{}) to {}#{}",
                milestone_title, milestone_number, target.repository, target.number
            ),
            serde_json::json!({
                "number": target.number,
                "target_repo": target.repository,
                "milestone_number": milestone_number,
                "milestone_title": milestone_title,
                "created": created,
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safe_outputs::ToolResult;
    use crate::secure::GithubTemporaryId;
    use std::collections::HashMap;
    use wiremock::matchers::{body_json, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn contract_and_parameter_validation() {
        assert_eq!(
            AssignGithubIssueMilestoneResult::NAME,
            "assign-github-issue-milestone"
        );
        assert_eq!(AssignGithubIssueMilestoneResult::DEFAULT_MAX, 1);
        assert!(
            AssignGithubIssueMilestoneParams {
                issue_number: GithubIssueNumber::Number(1),
                milestone_number: Some(2),
                milestone_title: None,
                repository: None,
            }
            .validate()
            .is_ok()
        );
        assert!(
            AssignGithubIssueMilestoneParams {
                issue_number: GithubIssueNumber::Temporary(
                    GithubTemporaryId::parse("#aw_created").unwrap()
                ),
                milestone_number: None,
                milestone_title: Some("Sprint 1".to_string()),
                repository: Some("octo/repo".to_string()),
            }
            .validate()
            .is_ok()
        );
        for params in [
            AssignGithubIssueMilestoneParams {
                issue_number: GithubIssueNumber::Number(1),
                milestone_number: None,
                milestone_title: None,
                repository: None,
            },
            AssignGithubIssueMilestoneParams {
                issue_number: GithubIssueNumber::Number(1),
                milestone_number: Some(2),
                milestone_title: Some("Sprint 1".to_string()),
                repository: None,
            },
        ] {
            assert!(params.validate().is_err());
        }
    }

    #[test]
    fn strict_config_supports_allowed_titles_and_numbers() {
        let config: AssignGithubIssueMilestoneConfig = serde_yaml::from_str(
            r#"
target-repo: octo/repo
allowed-repos: [octo/other]
required-labels: [managed]
required-title-prefix: "[agent]"
allowed: [3, "Release 1"]
auto-create: true
max: 1
"#,
        )
        .unwrap();
        assert_eq!(
            config.allowed,
            vec![
                GithubMilestoneAllowance::Number(3),
                GithubMilestoneAllowance::Title("Release 1".to_string())
            ]
        );
        assert!(config.auto_create);
        assert!(
            serde_yaml::from_str::<AssignGithubIssueMilestoneConfig>(
                "target-repo: octo/repo\nunexpected: true\n"
            )
            .is_err()
        );
    }

    fn context(server: &MockServer, config: serde_json::Value) -> ExecutionContext {
        let mut tool_configs = HashMap::new();
        tool_configs.insert("assign-github-issue-milestone".to_string(), config);
        ExecutionContext {
            github_token: Some("token".to_string()),
            github_api_url: server.uri(),
            tool_configs,
            ..Default::default()
        }
    }

    fn issue() -> serde_json::Value {
        serde_json::json!({
            "number": 7,
            "node_id": "I_7",
            "title": "[agent] tracked",
            "state": "open",
            "labels": [{"name": "managed"}]
        })
    }

    #[tokio::test]
    async fn resolves_allowed_numeric_milestone_across_pages() {
        let server = MockServer::start().await;
        let next = format!(
            "<{}/page2/milestones?state=all&per_page=100>; rel=\"next\"",
            server.uri()
        );
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue()))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/milestones"))
            .and(query_param("state", "all"))
            .and(query_param("per_page", "100"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Link", next)
                    .set_body_json(serde_json::json!([])),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/page2/milestones"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!([{
                    "number": 3,
                    "title": "Release 1",
                    "state": "open",
                    "node_id": "MI_3"
                }])),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/repos/octo/repo/issues/7"))
            .and(body_json(serde_json::json!({"milestone": 3})))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue()))
            .expect(1)
            .mount(&server)
            .await;

        let ctx = context(
            &server,
            serde_json::json!({
                "target-repo": "octo/repo",
                "required-labels": ["managed"],
                "required-title-prefix": "[agent]",
                "allowed": [3]
            }),
        );
        let mut result: AssignGithubIssueMilestoneResult = AssignGithubIssueMilestoneParams {
            issue_number: GithubIssueNumber::Number(7),
            milestone_number: Some(3),
            milestone_title: None,
            repository: None,
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(execution.success, "{}", execution.message);
        assert_eq!(
            execution.data.as_ref().unwrap()["milestone_title"],
            "Release 1"
        );
    }

    #[tokio::test]
    async fn auto_creates_allowed_title_after_all_preflight_reads() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue()))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/milestones"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/octo/repo/milestones"))
            .and(body_json(serde_json::json!({"title": "Release 2"})))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "number": 4,
                "title": "Release 2"
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/repos/octo/repo/issues/7"))
            .and(body_json(serde_json::json!({"milestone": 4})))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue()))
            .expect(1)
            .mount(&server)
            .await;

        let ctx = context(
            &server,
            serde_json::json!({
                "target-repo": "octo/repo",
                "allowed": ["Release 2"],
                "auto-create": true
            }),
        );
        let mut result: AssignGithubIssueMilestoneResult = AssignGithubIssueMilestoneParams {
            issue_number: GithubIssueNumber::Number(7),
            milestone_number: None,
            milestone_title: Some("Release 2".to_string()),
            repository: None,
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(execution.success, "{}", execution.message);
        assert_eq!(execution.data.as_ref().unwrap()["created"], true);
    }

    #[tokio::test]
    async fn failed_filter_performs_no_milestone_read_or_write() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue()))
            .expect(1)
            .mount(&server)
            .await;
        let ctx = context(
            &server,
            serde_json::json!({
                "target-repo": "octo/repo",
                "required-labels": ["missing"],
                "allowed": ["Release 2"],
                "auto-create": true
            }),
        );
        let mut result: AssignGithubIssueMilestoneResult = AssignGithubIssueMilestoneParams {
            issue_number: GithubIssueNumber::Number(7),
            milestone_number: None,
            milestone_title: Some("Release 2".to_string()),
            repository: None,
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method.as_str(), "GET");
    }

    #[tokio::test]
    async fn dry_run_needs_no_token_or_configuration() {
        let ctx = ExecutionContext {
            dry_run: true,
            ..Default::default()
        };
        let mut result: AssignGithubIssueMilestoneResult = AssignGithubIssueMilestoneParams {
            issue_number: GithubIssueNumber::Number(7),
            milestone_number: None,
            milestone_title: Some("Release 2".to_string()),
            repository: None,
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(execution.success);
        assert!(execution.message.contains("[DRY-RUN]"));
        assert!(execution.message.contains("Release 2"));
    }
}
