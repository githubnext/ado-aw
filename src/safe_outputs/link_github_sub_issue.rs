//! `link-github-sub-issue` safe output.

use anyhow::ensure;
use log::{debug, info};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::safe_outputs::{
    ExecutionContext, ExecutionResult, Executor, GithubClient, GithubIssueNumber,
    GithubMutationFilters, GithubRepositoryPolicy, GithubTargetCapabilities, Validate,
    resolve_github_issue_target, validate_github_mutation_filter_config,
    validate_github_mutation_filters, validate_github_repository,
    validate_github_target_capability,
};
use crate::sanitize::{SanitizeContent, sanitize_config};
use crate::tool_result;
use ado_aw_derive::SanitizeConfig;

const GET_SUB_ISSUE_PARENT: &str = r#"query($id: ID!) {
  node(id: $id) {
    ... on Issue {
      id
      parent {
        id
        number
        repository { nameWithOwner }
      }
    }
  }
}"#;

const ADD_SUB_ISSUE: &str = r#"mutation($parentId: ID!, $subIssueId: ID!) {
  addSubIssue(input: {
    issueId: $parentId
    subIssueId: $subIssueId
    replaceParent: false
  }) {
    issue { id number }
    subIssue { id number }
  }
}"#;

#[derive(Deserialize, JsonSchema)]
pub struct LinkGithubSubIssueParams {
    /// Positive parent issue number or a temporary ID from create-github-issue.
    pub parent_issue_number: GithubIssueNumber,
    /// Positive child issue number or a temporary ID from create-github-issue.
    pub sub_issue_number: GithubIssueNumber,
    /// Optional repository shared by the parent and child.
    #[serde(default)]
    pub repository: Option<String>,
}

impl Validate for LinkGithubSubIssueParams {
    fn validate(&self) -> anyhow::Result<()> {
        self.parent_issue_number.validate("parent_issue_number")?;
        self.sub_issue_number.validate("sub_issue_number")?;
        ensure!(
            !same_issue_reference(&self.parent_issue_number, &self.sub_issue_number),
            "parent_issue_number and sub_issue_number must be different"
        );
        if let Some(repository) = self.repository.as_deref() {
            validate_github_repository(repository)?;
        }
        Ok(())
    }
}

tool_result! {
    name = "link-github-sub-issue",
    write = true,
    params = LinkGithubSubIssueParams,
    default_max = 5,
    /// Result of linking two GitHub issues as parent and child.
    pub struct LinkGithubSubIssueResult {
        parent_issue_number: GithubIssueNumber,
        sub_issue_number: GithubIssueNumber,
        #[serde(default)]
        repository: Option<String>,
    }
}

impl SanitizeContent for LinkGithubSubIssueResult {
    fn sanitize_content_fields(&mut self) {
        self.repository = self.repository.as_deref().map(sanitize_config);
    }
}

#[derive(Debug, Clone, Default, SanitizeConfig, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkGithubSubIssueConfig {
    #[serde(default, rename = "target-repo")]
    pub target_repo: Option<String>,
    #[serde(default, rename = "allowed-repos")]
    pub allowed_repos: Vec<String>,
    #[serde(default, rename = "parent-required-labels")]
    pub parent_required_labels: Vec<String>,
    #[serde(default, rename = "parent-title-prefix")]
    pub parent_title_prefix: Option<String>,
    #[serde(default, rename = "sub-required-labels")]
    pub sub_required_labels: Vec<String>,
    #[serde(default, rename = "sub-title-prefix")]
    pub sub_title_prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[sanitize_config(skip)]
    pub max: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExistingParent {
    id: String,
    number: u64,
    repository: String,
}

#[async_trait::async_trait]
impl Executor for LinkGithubSubIssueResult {
    fn dry_run_summary(&self) -> String {
        format!(
            "link GitHub issue {} as a sub-issue of {}",
            display_issue_number(&self.sub_issue_number),
            display_issue_number(&self.parent_issue_number)
        )
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        if !ctx.tool_configs.contains_key("link-github-sub-issue") {
            return Ok(ExecutionResult::failure(
                "link-github-sub-issue is not configured for this workflow",
            ));
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
        let config: LinkGithubSubIssueConfig = ctx.get_tool_config("link-github-sub-issue")?;
        if let Err(error) = validate_link_github_sub_issue_config(&config) {
            return Ok(ExecutionResult::failure(error.to_string()));
        }
        let policy =
            GithubRepositoryPolicy::new(config.target_repo.as_deref(), &config.allowed_repos);
        let parent = match resolve_github_issue_target(
            &self.parent_issue_number,
            self.repository.as_deref(),
            policy,
            ctx,
        )? {
            Ok(target) => target,
            Err(result) => return Ok(result),
        };
        let sub_issue = match resolve_github_issue_target(
            &self.sub_issue_number,
            self.repository.as_deref(),
            policy,
            ctx,
        )? {
            Ok(target) => target,
            Err(result) => return Ok(result),
        };
        if !parent
            .repository
            .eq_ignore_ascii_case(&sub_issue.repository)
        {
            return Ok(ExecutionResult::failure(format!(
                "parent issue repository '{}' and sub-issue repository '{}' must be the same",
                parent.repository, sub_issue.repository
            )));
        }
        if parent.number == sub_issue.number {
            return Ok(ExecutionResult::failure(
                "parent_issue_number and sub_issue_number resolved to the same GitHub issue",
            ));
        }

        let client = GithubClient::new(&ctx.github_api_url, token)?;
        let parent_metadata = match client.get_issue(&parent.repository, parent.number).await? {
            Ok(metadata) => metadata,
            Err(error) => return Ok(ExecutionResult::failure(error.to_string())),
        };
        let sub_metadata = match client
            .get_issue(&sub_issue.repository, sub_issue.number)
            .await?
        {
            Ok(metadata) => metadata,
            Err(error) => return Ok(ExecutionResult::failure(error.to_string())),
        };
        for metadata in [&parent_metadata, &sub_metadata] {
            if let Err(result) =
                validate_github_target_capability(metadata, GithubTargetCapabilities::ISSUES_ONLY)
            {
                return Ok(result);
            }
        }
        let parent_filters = GithubMutationFilters {
            required_labels: &config.parent_required_labels,
            required_title_prefix: config.parent_title_prefix.as_deref(),
        };
        let sub_filters = GithubMutationFilters {
            required_labels: &config.sub_required_labels,
            required_title_prefix: config.sub_title_prefix.as_deref(),
        };
        if let Err(result) = validate_github_mutation_filters(&parent_metadata, parent_filters) {
            return Ok(result);
        }
        if let Err(result) = validate_github_mutation_filters(&sub_metadata, sub_filters) {
            return Ok(result);
        }
        let Some(parent_node_id) = parent_metadata.node_id.as_deref() else {
            return Ok(ExecutionResult::failure(format!(
                "GitHub parent issue {}#{} has no GraphQL node ID; sub-issues are unsupported or unavailable",
                parent.repository, parent.number
            )));
        };
        let Some(sub_node_id) = sub_metadata.node_id.as_deref() else {
            return Ok(ExecutionResult::failure(format!(
                "GitHub sub-issue {}#{} has no GraphQL node ID; sub-issues are unsupported or unavailable",
                sub_issue.repository, sub_issue.number
            )));
        };

        let preflight = match client
            .graphql(
                "Check GitHub sub-issue parent",
                GET_SUB_ISSUE_PARENT,
                serde_json::json!({ "id": sub_node_id }),
            )
            .await?
        {
            Ok(data) => data,
            Err(error) => {
                return Ok(ExecutionResult::failure(format!(
                    "GitHub sub-issues are unsupported or unavailable: {error}"
                )));
            }
        };
        if let Some(existing) = match parse_existing_parent(&preflight) {
            Ok(parent) => parent,
            Err(message) => return Ok(ExecutionResult::failure(message)),
        } {
            let same_parent = existing.id == parent_node_id;
            if same_parent {
                info!(
                    "GitHub issue {}#{} is already a sub-issue of #{}",
                    parent.repository, sub_issue.number, parent.number
                );
                return Ok(ExecutionResult::success_with_data(
                    format!(
                        "GitHub issue {}#{} is already a sub-issue of #{}",
                        parent.repository, sub_issue.number, parent.number
                    ),
                    serde_json::json!({
                        "parent_issue_number": parent.number,
                        "sub_issue_number": sub_issue.number,
                        "target_repo": parent.repository,
                        "already_linked": true,
                    }),
                ));
            }
            let existing_target = format!("{}#{}", existing.repository, existing.number);
            return Ok(ExecutionResult::failure(format!(
                "GitHub issue {}#{} is already linked to a different parent ({existing_target}); refusing to replace it",
                sub_issue.repository, sub_issue.number
            )));
        }

        debug!(
            "Linking GitHub issue {}#{} as a sub-issue of #{}",
            parent.repository, sub_issue.number, parent.number
        );
        let mutation = match client
            .graphql(
                "Link GitHub sub-issue",
                ADD_SUB_ISSUE,
                serde_json::json!({
                    "parentId": parent_node_id,
                    "subIssueId": sub_node_id,
                }),
            )
            .await?
        {
            Ok(data) => data,
            Err(error) => {
                return Ok(ExecutionResult::failure(format!(
                    "GitHub addSubIssue mutation is unsupported or failed: {error}"
                )));
            }
        };
        let mutated_parent = mutation
            .pointer("/addSubIssue/issue/number")
            .and_then(Value::as_u64);
        let mutated_sub = mutation
            .pointer("/addSubIssue/subIssue/number")
            .and_then(Value::as_u64);
        if mutated_parent != Some(parent.number) || mutated_sub != Some(sub_issue.number) {
            return Ok(ExecutionResult::failure(
                "GitHub addSubIssue response did not identify the requested parent and sub-issue",
            ));
        }

        info!(
            "Linked GitHub issue {}#{} as a sub-issue of #{}",
            parent.repository, sub_issue.number, parent.number
        );
        Ok(ExecutionResult::success_with_data(
            format!(
                "Linked GitHub issue {}#{} as a sub-issue of #{}",
                parent.repository, sub_issue.number, parent.number
            ),
            serde_json::json!({
                "parent_issue_number": parent.number,
                "sub_issue_number": sub_issue.number,
                "target_repo": parent.repository,
                "already_linked": false,
            }),
        ))
    }
}

pub(crate) fn validate_link_github_sub_issue_config(
    config: &LinkGithubSubIssueConfig,
) -> anyhow::Result<()> {
    validate_github_mutation_filter_config(GithubMutationFilters {
        required_labels: &config.parent_required_labels,
        required_title_prefix: config.parent_title_prefix.as_deref(),
    })?;
    validate_github_mutation_filter_config(GithubMutationFilters {
        required_labels: &config.sub_required_labels,
        required_title_prefix: config.sub_title_prefix.as_deref(),
    })
}

fn same_issue_reference(parent: &GithubIssueNumber, sub_issue: &GithubIssueNumber) -> bool {
    match (parent, sub_issue) {
        (GithubIssueNumber::Number(parent), GithubIssueNumber::Number(sub_issue)) => {
            parent == sub_issue
        }
        (GithubIssueNumber::Temporary(parent), GithubIssueNumber::Temporary(sub_issue)) => {
            parent.canonical() == sub_issue.canonical()
        }
        _ => false,
    }
}

fn display_issue_number(issue_number: &GithubIssueNumber) -> String {
    match issue_number {
        GithubIssueNumber::Number(number) => format!("#{number}"),
        GithubIssueNumber::Temporary(temporary_id) => temporary_id.canonical(),
    }
}

fn parse_existing_parent(data: &Value) -> Result<Option<ExistingParent>, String> {
    let node = data.get("node").and_then(Value::as_object).ok_or_else(|| {
        "GitHub sub-issue preflight did not return the requested issue; sub-issues may be unsupported"
            .to_string()
    })?;
    let Some(parent) = node.get("parent") else {
        return Err(
            "GitHub API response did not expose Issue.parent; sub-issues are unsupported"
                .to_string(),
        );
    };
    if parent.is_null() {
        return Ok(None);
    }
    let parent = parent.as_object().ok_or_else(|| {
        "GitHub sub-issue preflight returned malformed parent metadata".to_string()
    })?;
    let id = parent
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| "GitHub sub-issue parent had no GraphQL node ID".to_string())?;
    let number = parent
        .get("number")
        .and_then(Value::as_u64)
        .filter(|number| *number > 0)
        .ok_or_else(|| "GitHub sub-issue parent had no positive issue number".to_string())?;
    let repository = parent
        .get("repository")
        .and_then(Value::as_object)
        .and_then(|repository| repository.get("nameWithOwner"))
        .and_then(Value::as_str)
        .filter(|repository| !repository.is_empty())
        .ok_or_else(|| "GitHub sub-issue parent had no repository identity".to_string())?;
    Ok(Some(ExistingParent {
        id: id.to_string(),
        number,
        repository: repository.to_string(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safe_outputs::ToolResult;
    use crate::secure::GithubTemporaryId;
    use std::collections::HashMap;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn result_contract_and_dry_run() {
        assert_eq!(LinkGithubSubIssueResult::NAME, "link-github-sub-issue");
        assert_eq!(LinkGithubSubIssueResult::DEFAULT_MAX, 5);
        let result: LinkGithubSubIssueResult = LinkGithubSubIssueParams {
            parent_issue_number: GithubIssueNumber::Number(10),
            sub_issue_number: GithubIssueNumber::Number(11),
            repository: None,
        }
        .try_into()
        .unwrap();
        assert_eq!(
            result.dry_run_summary(),
            "link GitHub issue #11 as a sub-issue of #10"
        );
    }

    #[test]
    fn validates_numeric_and_temporary_references() {
        let params = LinkGithubSubIssueParams {
            parent_issue_number: GithubIssueNumber::Temporary(
                GithubTemporaryId::parse("#aw_parent").unwrap(),
            ),
            sub_issue_number: GithubIssueNumber::Temporary(
                GithubTemporaryId::parse("#aw_sub").unwrap(),
            ),
            repository: Some("octo/repo".to_string()),
        };
        assert!(params.validate().is_ok());

        let same = LinkGithubSubIssueParams {
            parent_issue_number: GithubIssueNumber::Number(7),
            sub_issue_number: GithubIssueNumber::Number(7),
            repository: None,
        };
        assert!(
            same.validate()
                .unwrap_err()
                .to_string()
                .contains("must be different")
        );
    }

    #[test]
    fn rejects_same_temporary_id_and_invalid_repository() {
        let temporary_id = GithubTemporaryId::parse("#aw_same").unwrap();
        let same = LinkGithubSubIssueParams {
            parent_issue_number: GithubIssueNumber::Temporary(temporary_id.clone()),
            sub_issue_number: GithubIssueNumber::Temporary(temporary_id),
            repository: None,
        };
        assert!(same.validate().is_err());

        let invalid_repo = LinkGithubSubIssueParams {
            parent_issue_number: GithubIssueNumber::Number(1),
            sub_issue_number: GithubIssueNumber::Number(2),
            repository: Some("octo/$(TOKEN)".to_string()),
        };
        assert!(invalid_repo.validate().is_err());
    }

    #[test]
    fn config_is_strict_and_has_separate_filters() {
        assert!(
            serde_yaml::from_str::<LinkGithubSubIssueConfig>(
                "parent-required-labels: [parent]\nunknown: true"
            )
            .is_err()
        );
        let config: LinkGithubSubIssueConfig = serde_yaml::from_str(
            "parent-required-labels: [parent]\n\
             parent-title-prefix: 'Parent: '\n\
             sub-required-labels: [child]\n\
             sub-title-prefix: 'Child: '\n\
             max: 4",
        )
        .unwrap();
        assert_eq!(config.parent_required_labels, vec!["parent"]);
        assert_eq!(config.parent_title_prefix.as_deref(), Some("Parent: "));
        assert_eq!(config.sub_required_labels, vec!["child"]);
        assert_eq!(config.sub_title_prefix.as_deref(), Some("Child: "));
        assert_eq!(config.max, Some(4));
        assert!(validate_link_github_sub_issue_config(&config).is_ok());
    }

    #[test]
    fn parses_none_same_and_different_parent_metadata() {
        assert_eq!(
            parse_existing_parent(&serde_json::json!({
                "node": {"id": "SUB", "parent": null}
            }))
            .unwrap(),
            None
        );
        assert_eq!(
            parse_existing_parent(&serde_json::json!({
                "node": {
                    "id": "SUB",
                    "parent": {
                        "id": "PARENT",
                        "number": 10,
                        "repository": {"nameWithOwner": "octo/repo"}
                    }
                }
            }))
            .unwrap(),
            Some(ExistingParent {
                id: "PARENT".to_string(),
                number: 10,
                repository: "octo/repo".to_string(),
            })
        );
        assert!(
            parse_existing_parent(&serde_json::json!({"node": {"id": "SUB"}}))
                .unwrap_err()
                .contains("unsupported")
        );
    }

    async fn mount_issue(
        server: &MockServer,
        number: u64,
        node_id: &str,
        title: &str,
        label: &str,
    ) {
        Mock::given(method("GET"))
            .and(path(format!("/repos/octo/repo/issues/{number}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "number": number,
                "node_id": node_id,
                "title": title,
                "state": "open",
                "labels": [{"name": label}],
                "html_url": format!("https://github.example/octo/repo/issues/{number}")
            })))
            .expect(1)
            .mount(server)
            .await;
    }

    fn context(server: &MockServer) -> ExecutionContext {
        let mut tool_configs = HashMap::new();
        tool_configs.insert(
            "link-github-sub-issue".to_string(),
            serde_json::json!({
                "target-repo": "octo/repo",
                "parent-required-labels": ["parent"],
                "parent-title-prefix": "Parent:",
                "sub-required-labels": ["child"],
                "sub-title-prefix": "Child:"
            }),
        );
        ExecutionContext {
            github_token: Some("token".to_string()),
            github_api_url: server.uri(),
            tool_configs,
            ..Default::default()
        }
    }

    fn result() -> LinkGithubSubIssueResult {
        LinkGithubSubIssueParams {
            parent_issue_number: GithubIssueNumber::Number(10),
            sub_issue_number: GithubIssueNumber::Number(11),
            repository: None,
        }
        .try_into()
        .unwrap()
    }

    #[tokio::test]
    async fn preflights_filters_and_links_sub_issue() {
        let server = MockServer::start().await;
        mount_issue(&server, 10, "PARENT", "Parent: plan", "parent").await;
        mount_issue(&server, 11, "SUB", "Child: task", "child").await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_json(serde_json::json!({
                "query": GET_SUB_ISSUE_PARENT,
                "variables": {"id": "SUB"}
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {"node": {"id": "SUB", "parent": null}}
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_json(serde_json::json!({
                "query": ADD_SUB_ISSUE,
                "variables": {"parentId": "PARENT", "subIssueId": "SUB"}
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {
                    "addSubIssue": {
                        "issue": {"id": "PARENT", "number": 10},
                        "subIssue": {"id": "SUB", "number": 11}
                    }
                }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let ctx = context(&server);
        let mut result = result();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(
            execution.success,
            "unexpected failure: {}",
            execution.message
        );
        assert_eq!(
            execution
                .data
                .as_ref()
                .and_then(|data| data["already_linked"].as_bool()),
            Some(false)
        );
    }

    #[tokio::test]
    async fn existing_same_parent_is_idempotent_without_mutation() {
        let server = MockServer::start().await;
        mount_issue(&server, 10, "PARENT", "Parent: plan", "parent").await;
        mount_issue(&server, 11, "SUB", "Child: task", "child").await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_json(serde_json::json!({
                "query": GET_SUB_ISSUE_PARENT,
                "variables": {"id": "SUB"}
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {
                    "node": {
                        "id": "SUB",
                        "parent": {
                            "id": "PARENT",
                            "number": 10,
                            "repository": {"nameWithOwner": "octo/repo"}
                        }
                    }
                }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let ctx = context(&server);
        let mut result = result();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(execution.success);
        assert_eq!(
            execution
                .data
                .as_ref()
                .and_then(|data| data["already_linked"].as_bool()),
            Some(true)
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn different_existing_parent_fails_without_mutation() {
        let server = MockServer::start().await;
        mount_issue(&server, 10, "PARENT", "Parent: plan", "parent").await;
        mount_issue(&server, 11, "SUB", "Child: task", "child").await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {
                    "node": {
                        "id": "SUB",
                        "parent": {
                            "id": "OTHER",
                            "number": 9,
                            "repository": {"nameWithOwner": "octo/repo"}
                        }
                    }
                }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let ctx = context(&server);
        let mut result = result();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("different parent"));
        assert_eq!(server.received_requests().await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn unsupported_parent_query_fails_explicitly() {
        let server = MockServer::start().await;
        mount_issue(&server, 10, "PARENT", "Parent: plan", "parent").await;
        mount_issue(&server, 11, "SUB", "Child: task", "child").await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": null,
                "errors": [{
                    "type": "undefinedField",
                    "message": "Field 'parent' doesn't exist on type 'Issue'"
                }]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let ctx = context(&server);
        let mut result = result();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("unsupported or unavailable"));
    }
}
