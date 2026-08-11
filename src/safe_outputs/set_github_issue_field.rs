//! `set-github-issue-field` safe output.

use anyhow::ensure;
use chrono::NaiveDate;
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
use crate::validate::reject_pipeline_injection;
use ado_aw_derive::SanitizeConfig;

const DISCOVER_ISSUE_FIELDS: &str = r#"query($owner: String!, $repo: String!) {
  repository(owner: $owner, name: $repo) {
    issueFields(first: 100) {
      nodes {
        __typename
        ... on IssueFieldText { id name }
        ... on IssueFieldNumber { id name }
        ... on IssueFieldDate { id name }
        ... on IssueFieldSingleSelect { id name options { id name } }
        ... on IssueFieldMultiSelect { id name options { id name } }
      }
    }
  }
}"#;

const SET_ISSUE_FIELD_VALUE: &str = r#"mutation($issueId: ID!, $issueFields: [IssueFieldCreateOrUpdateInput!]!) {
  setIssueFieldValue(input: { issueId: $issueId, issueFields: $issueFields }) {
    issue { id number }
  }
}"#;

#[derive(Deserialize, JsonSchema)]
pub struct SetGithubIssueFieldParams {
    /// Positive GitHub issue number or a temporary ID from create-github-issue.
    pub issue_number: GithubIssueNumber,
    /// Exact repository-defined field name.
    #[serde(default)]
    pub field_name: Option<String>,
    /// GraphQL node ID of a repository-defined field.
    #[serde(default)]
    pub field_node_id: Option<String>,
    /// String representation of the desired field value.
    pub value: String,
    /// Optional target repository.
    #[serde(default)]
    pub repository: Option<String>,
}

impl Validate for SetGithubIssueFieldParams {
    fn validate(&self) -> anyhow::Result<()> {
        self.issue_number.validate("issue_number")?;
        ensure!(
            self.field_name.is_some() ^ self.field_node_id.is_some(),
            "exactly one of field_name or field_node_id must be provided"
        );
        if let Some(field_name) = self.field_name.as_deref() {
            validate_field_selector(field_name, "field_name")?;
            ensure!(
                !is_builtin_issue_field(field_name),
                "field_name '{}' is a built-in GitHub issue field; use its dedicated safe-output tool",
                field_name
            );
        }
        if let Some(field_node_id) = self.field_node_id.as_deref() {
            validate_field_selector(field_node_id, "field_node_id")?;
        }
        ensure!(
            self.value.len() <= 65_536,
            "value must be 65536 characters or fewer"
        );
        reject_pipeline_injection(&self.value, "set-github-issue-field.value")?;
        if let Some(repository) = self.repository.as_deref() {
            validate_github_repository(repository)?;
        }
        Ok(())
    }
}

tool_result! {
    name = "set-github-issue-field",
    write = true,
    params = SetGithubIssueFieldParams,
    default_max = 5,
    /// Result of setting a repository-defined GitHub issue field.
    pub struct SetGithubIssueFieldResult {
        issue_number: GithubIssueNumber,
        #[serde(default)]
        field_name: Option<String>,
        #[serde(default)]
        field_node_id: Option<String>,
        value: String,
        #[serde(default)]
        repository: Option<String>,
    }
}

impl SanitizeContent for SetGithubIssueFieldResult {
    fn sanitize_content_fields(&mut self) {
        self.field_name = self.field_name.as_deref().map(sanitize_config);
        self.field_node_id = self.field_node_id.as_deref().map(sanitize_config);
        // Field values are external API payloads. Preserve their exact semantic
        // value while removing transport controls and ADO logging commands.
        self.value = sanitize_config(&self.value);
        self.repository = self.repository.as_deref().map(sanitize_config);
    }
}

#[derive(Debug, Clone, Default, SanitizeConfig, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetGithubIssueFieldConfig {
    #[serde(default, rename = "target-repo")]
    pub target_repo: Option<String>,
    #[serde(default, rename = "allowed-repos")]
    pub allowed_repos: Vec<String>,
    #[serde(default, rename = "required-labels")]
    pub required_labels: Vec<String>,
    #[serde(default, rename = "required-title-prefix")]
    pub required_title_prefix: Option<String>,
    #[serde(default, rename = "allowed-fields")]
    pub allowed_fields: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[sanitize_config(skip)]
    pub max: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
struct IssueField {
    id: String,
    name: String,
    kind: String,
    options: Vec<IssueFieldOption>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IssueFieldOption {
    id: String,
    name: String,
}

#[async_trait::async_trait]
impl Executor for SetGithubIssueFieldResult {
    fn dry_run_summary(&self) -> String {
        let target = display_issue_number(&self.issue_number);
        let field = self
            .field_name
            .as_deref()
            .or(self.field_node_id.as_deref())
            .unwrap_or("<missing field>");
        format!(
            "set GitHub issue field '{field}' on {target} to '{}'",
            self.value
        )
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        if !ctx.tool_configs.contains_key("set-github-issue-field") {
            return Ok(ExecutionResult::failure(
                "set-github-issue-field is not configured for this workflow",
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
        let config: SetGithubIssueFieldConfig = ctx.get_tool_config("set-github-issue-field")?;
        if let Err(error) = validate_set_github_issue_field_config(&config) {
            return Ok(ExecutionResult::failure(error.to_string()));
        }

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
        let metadata = match client.get_issue(&target.repository, target.number).await? {
            Ok(metadata) => metadata,
            Err(error) => return Ok(ExecutionResult::failure(error.to_string())),
        };
        if let Err(result) =
            validate_github_target_capability(&metadata, GithubTargetCapabilities::ISSUES_ONLY)
        {
            return Ok(result);
        }
        let filters = GithubMutationFilters {
            required_labels: &config.required_labels,
            required_title_prefix: config.required_title_prefix.as_deref(),
        };
        if let Err(error) = validate_github_mutation_filter_config(filters) {
            return Ok(ExecutionResult::failure(error.to_string()));
        }
        if let Err(result) = validate_github_mutation_filters(&metadata, filters) {
            return Ok(result);
        }
        let Some(issue_node_id) = metadata.node_id.as_deref() else {
            return Ok(ExecutionResult::failure(format!(
                "GitHub issue {}#{} has no GraphQL node ID; issue fields are unsupported or unavailable",
                target.repository, target.number
            )));
        };

        let (owner, repo) = target
            .repository
            .split_once('/')
            .expect("resolved GitHub repository is validated");
        let discovery = match client
            .graphql(
                "Discover GitHub issue fields",
                DISCOVER_ISSUE_FIELDS,
                serde_json::json!({ "owner": owner, "repo": repo }),
            )
            .await?
        {
            Ok(data) => data,
            Err(error) => {
                return Ok(ExecutionResult::failure(format!(
                    "GitHub issue fields are unsupported or unavailable: {error}"
                )));
            }
        };
        let fields = match parse_issue_fields(&discovery) {
            Ok(fields) => fields,
            Err(message) => return Ok(ExecutionResult::failure(message)),
        };
        let field = match select_issue_field(
            &fields,
            self.field_name.as_deref(),
            self.field_node_id.as_deref(),
        ) {
            Ok(field) => field,
            Err(message) => return Ok(ExecutionResult::failure(message)),
        };
        if is_builtin_issue_field(&field.name) {
            return Ok(ExecutionResult::failure(format!(
                "GitHub field '{}' is a built-in issue field; use its dedicated safe-output tool",
                crate::sanitize::neutralize_pipeline_commands(&field.name)
            )));
        }
        if !github_issue_field_is_allowed(&config.allowed_fields, &field.name) {
            return Ok(ExecutionResult::failure(format!(
                "GitHub issue field '{}' is not in allowed-fields: {}",
                crate::sanitize::neutralize_pipeline_commands(&field.name),
                config.allowed_fields.join(", ")
            )));
        }

        let field_input = match coerce_field_value(field, &self.value) {
            Ok(input) => input,
            Err(message) => return Ok(ExecutionResult::failure(message)),
        };
        debug!(
            "Setting GitHub issue field {} ({}) on {}#{}",
            field.name, field.kind, target.repository, target.number
        );
        let mutation = match client
            .graphql(
                "Set GitHub issue field value",
                SET_ISSUE_FIELD_VALUE,
                serde_json::json!({
                    "issueId": issue_node_id,
                    "issueFields": [field_input],
                }),
            )
            .await?
        {
            Ok(data) => data,
            Err(error) => {
                return Ok(ExecutionResult::failure(format!(
                    "GitHub issue field mutation is unsupported or failed: {error}"
                )));
            }
        };
        let updated_number = mutation
            .pointer("/setIssueFieldValue/issue/number")
            .and_then(Value::as_u64);
        if updated_number != Some(target.number) {
            return Ok(ExecutionResult::failure(
                "GitHub setIssueFieldValue response did not identify the updated issue",
            ));
        }

        info!(
            "Set GitHub issue field '{}' on {}#{}",
            field.name, target.repository, target.number
        );
        Ok(ExecutionResult::success_with_data(
            format!(
                "Set GitHub issue field '{}' on {}#{}",
                field.name, target.repository, target.number
            ),
            serde_json::json!({
                "number": target.number,
                "target_repo": target.repository,
                "field_name": field.name,
                "field_node_id": field.id,
                "field_type": field.kind,
                "value": self.value,
            }),
        ))
    }
}

fn github_issue_field_is_allowed(allowed_fields: &[String], field_name: &str) -> bool {
    allowed_fields
        .iter()
        .any(|allowed| allowed == "*" || allowed.eq_ignore_ascii_case(field_name))
}

fn validate_field_selector(value: &str, field: &str) -> anyhow::Result<()> {
    ensure!(!value.is_empty(), "{field} must not be empty");
    ensure!(
        value.len() <= 256,
        "{field} must be 256 characters or fewer"
    );
    reject_pipeline_injection(value, &format!("set-github-issue-field.{field}"))
}

pub(crate) fn validate_set_github_issue_field_config(
    config: &SetGithubIssueFieldConfig,
) -> anyhow::Result<()> {
    ensure!(
        !config.allowed_fields.is_empty(),
        "set-github-issue-field requires at least one allowed-fields entry"
    );
    for field in &config.allowed_fields {
        validate_field_selector(field, "allowed-fields")?;
        ensure!(
            !is_builtin_issue_field(field),
            "allowed-fields entry '{}' is a built-in GitHub issue field",
            field
        );
    }
    Ok(())
}

fn display_issue_number(issue_number: &GithubIssueNumber) -> String {
    match issue_number {
        GithubIssueNumber::Number(number) => format!("#{number}"),
        GithubIssueNumber::Temporary(temporary_id) => temporary_id.canonical(),
    }
}

fn normalized_field_name(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '-' | '_' => ' ',
            other => other.to_ascii_lowercase(),
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_builtin_issue_field(value: &str) -> bool {
    matches!(
        normalized_field_name(value).as_str(),
        "title"
            | "body"
            | "description"
            | "status"
            | "state"
            | "assignee"
            | "assignees"
            | "label"
            | "labels"
            | "type"
            | "issue type"
            | "milestone"
            | "project"
            | "projects"
            | "repository"
            | "relationship"
            | "relationships"
            | "development"
            | "parent issue"
            | "sub issue"
            | "sub issues"
    )
}

fn parse_issue_fields(data: &Value) -> Result<Vec<IssueField>, String> {
    let repository = data
        .get("repository")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            "GitHub repository was not found or repository issue fields are unsupported".to_string()
        })?;
    let connection = repository
        .get("issueFields")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            "GitHub repository/API does not expose the issueFields GraphQL feature".to_string()
        })?;
    let nodes = connection
        .get("nodes")
        .and_then(Value::as_array)
        .ok_or_else(|| "GitHub issueFields response contained no nodes array".to_string())?;
    let mut fields = Vec::with_capacity(nodes.len());
    for node in nodes {
        let object = node
            .as_object()
            .ok_or_else(|| "GitHub issueFields response contained a malformed field".to_string())?;
        let id = object.get("id").and_then(Value::as_str).ok_or_else(|| {
            "GitHub issueFields response contained a field without an ID".to_string()
        })?;
        let name = object.get("name").and_then(Value::as_str).ok_or_else(|| {
            "GitHub issueFields response contained a field without a name".to_string()
        })?;
        let kind = object
            .get("__typename")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                "GitHub issueFields response contained a field without a type".to_string()
            })?;
        let options = object
            .get("options")
            .and_then(Value::as_array)
            .map(|options| {
                options
                    .iter()
                    .filter_map(|option| {
                        Some(IssueFieldOption {
                            id: option.get("id")?.as_str()?.to_string(),
                            name: option.get("name")?.as_str()?.to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        fields.push(IssueField {
            id: id.to_string(),
            name: name.to_string(),
            kind: kind.to_string(),
            options,
        });
    }
    Ok(fields)
}

fn select_issue_field<'a>(
    fields: &'a [IssueField],
    field_name: Option<&str>,
    field_node_id: Option<&str>,
) -> Result<&'a IssueField, String> {
    let matches: Vec<&IssueField> = if let Some(name) = field_name {
        fields
            .iter()
            .filter(|field| field.name.eq_ignore_ascii_case(name))
            .collect()
    } else if let Some(id) = field_node_id {
        fields.iter().filter(|field| field.id == id).collect()
    } else {
        Vec::new()
    };
    match matches.as_slice() {
        [field] => Ok(*field),
        [] => {
            let selector = field_name.or(field_node_id).unwrap_or("<missing>");
            Err(format!(
                "No repository-defined GitHub issue field matched '{}'",
                crate::sanitize::neutralize_pipeline_commands(selector)
            ))
        }
        _ => Err(format!(
            "Multiple repository-defined GitHub issue fields matched '{}'; use field_node_id",
            crate::sanitize::neutralize_pipeline_commands(field_name.unwrap_or("<missing>"))
        )),
    }
}

fn coerce_field_value(field: &IssueField, value: &str) -> Result<Value, String> {
    let mut input = serde_json::Map::new();
    input.insert("fieldId".to_string(), Value::String(field.id.clone()));
    match field.kind.as_str() {
        "IssueFieldText" => {
            input.insert("textValue".to_string(), Value::String(value.to_string()));
        }
        "IssueFieldNumber" => {
            let number = value.parse::<f64>().map_err(|_| {
                format!(
                    "Value '{}' is not a valid number for GitHub issue field '{}'",
                    crate::sanitize::neutralize_pipeline_commands(value),
                    field.name
                )
            })?;
            if !number.is_finite() {
                return Err(format!(
                    "Value '{}' is not a finite number for GitHub issue field '{}'",
                    crate::sanitize::neutralize_pipeline_commands(value),
                    field.name
                ));
            }
            input.insert(
                "numberValue".to_string(),
                serde_json::Number::from_f64(number)
                    .map(Value::Number)
                    .expect("finite f64 converts to a JSON number"),
            );
        }
        "IssueFieldDate" => {
            NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| {
                format!(
                    "Value '{}' is not a valid YYYY-MM-DD date for GitHub issue field '{}'",
                    crate::sanitize::neutralize_pipeline_commands(value),
                    field.name
                )
            })?;
            input.insert("dateValue".to_string(), Value::String(value.to_string()));
        }
        "IssueFieldSingleSelect" => {
            let matching: Vec<&IssueFieldOption> = field
                .options
                .iter()
                .filter(|option| option.name.eq_ignore_ascii_case(value))
                .collect();
            let option = match matching.as_slice() {
                [option] => *option,
                [] => {
                    return Err(format!(
                        "Value '{}' is not an option for GitHub issue field '{}'; allowed options: {}",
                        crate::sanitize::neutralize_pipeline_commands(value),
                        field.name,
                        field
                            .options
                            .iter()
                            .map(|option| option.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                _ => {
                    return Err(format!(
                        "Multiple options named '{}' exist for GitHub issue field '{}'",
                        crate::sanitize::neutralize_pipeline_commands(value),
                        field.name
                    ));
                }
            };
            input.insert(
                "singleSelectOptionId".to_string(),
                Value::String(option.id.clone()),
            );
        }
        unsupported => {
            return Err(format!(
                "GitHub issue field '{}' uses unsupported type '{}'; supported types are single-select, number, date, and text",
                field.name,
                crate::sanitize::neutralize_pipeline_commands(unsupported)
            ));
        }
    }
    Ok(Value::Object(input))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safe_outputs::ToolResult;
    use crate::secure::GithubTemporaryId;
    use std::collections::HashMap;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn field(kind: &str) -> IssueField {
        IssueField {
            id: "IF_1".to_string(),
            name: "Priority".to_string(),
            kind: kind.to_string(),
            options: vec![
                IssueFieldOption {
                    id: "OPT_HIGH".to_string(),
                    name: "High".to_string(),
                },
                IssueFieldOption {
                    id: "OPT_LOW".to_string(),
                    name: "Low".to_string(),
                },
            ],
        }
    }

    #[test]
    fn result_contract_and_dry_run() {
        assert_eq!(SetGithubIssueFieldResult::NAME, "set-github-issue-field");
        assert_eq!(SetGithubIssueFieldResult::DEFAULT_MAX, 5);
        let result: SetGithubIssueFieldResult = SetGithubIssueFieldParams {
            issue_number: GithubIssueNumber::Number(7),
            field_name: Some("Priority".to_string()),
            field_node_id: None,
            value: "High".to_string(),
            repository: None,
        }
        .try_into()
        .unwrap();
        assert_eq!(
            result.dry_run_summary(),
            "set GitHub issue field 'Priority' on #7 to 'High'"
        );
    }

    #[test]
    fn validates_temporary_id_and_exactly_one_selector() {
        let valid = SetGithubIssueFieldParams {
            issue_number: GithubIssueNumber::Temporary(
                GithubTemporaryId::parse("#aw_field").unwrap(),
            ),
            field_name: None,
            field_node_id: Some("IF_1".to_string()),
            value: "42".to_string(),
            repository: Some("octo/repo".to_string()),
        };
        assert!(valid.validate().is_ok());

        for (name, id) in [(None, None), (Some("Priority"), Some("IF_1"))] {
            let invalid = SetGithubIssueFieldParams {
                issue_number: GithubIssueNumber::Number(1),
                field_name: name.map(str::to_string),
                field_node_id: id.map(str::to_string),
                value: "High".to_string(),
                repository: None,
            };
            assert!(
                invalid
                    .validate()
                    .unwrap_err()
                    .to_string()
                    .contains("exactly one")
            );
        }
    }

    #[test]
    fn rejects_built_in_field_and_pipeline_injection() {
        let built_in = SetGithubIssueFieldParams {
            issue_number: GithubIssueNumber::Number(1),
            field_name: Some("Issue_Type".to_string()),
            field_node_id: None,
            value: "Bug".to_string(),
            repository: None,
        };
        assert!(
            built_in
                .validate()
                .unwrap_err()
                .to_string()
                .contains("built-in")
        );

        let injected = SetGithubIssueFieldParams {
            issue_number: GithubIssueNumber::Number(1),
            field_name: Some("Priority".to_string()),
            field_node_id: None,
            value: "##vso[task.complete]".to_string(),
            repository: None,
        };
        assert!(injected.validate().is_err());
    }

    #[test]
    fn config_is_strict_and_requires_allowed_fields() {
        assert!(
            serde_yaml::from_str::<SetGithubIssueFieldConfig>(
                "allowed-fields: [Priority]\nunexpected: true"
            )
            .is_err()
        );
        assert!(
            validate_set_github_issue_field_config(&SetGithubIssueFieldConfig::default())
                .unwrap_err()
                .to_string()
                .contains("allowed-fields")
        );
        let config: SetGithubIssueFieldConfig =
            serde_yaml::from_str("allowed-fields: [Priority]\nmax: 3").unwrap();
        assert_eq!(config.allowed_fields, vec!["Priority"]);
        assert_eq!(config.max, Some(3));
        let wildcard: SetGithubIssueFieldConfig =
            serde_yaml::from_str("allowed-fields: ['*']").unwrap();
        assert!(validate_set_github_issue_field_config(&wildcard).is_ok());
        assert!(github_issue_field_is_allowed(
            &wildcard.allowed_fields,
            "Custom Priority"
        ));
        assert!(!github_issue_field_is_allowed(
            &config.allowed_fields,
            "Custom Priority"
        ));
    }

    #[test]
    fn parses_and_selects_discovered_fields() {
        let data = serde_json::json!({
            "repository": {
                "issueFields": {
                    "nodes": [{
                        "__typename": "IssueFieldSingleSelect",
                        "id": "IF_1",
                        "name": "Priority",
                        "options": [{"id": "OPT_HIGH", "name": "High"}]
                    }]
                }
            }
        });
        let fields = parse_issue_fields(&data).unwrap();
        assert_eq!(fields.len(), 1);
        assert_eq!(
            select_issue_field(&fields, Some("priority"), None)
                .unwrap()
                .id,
            "IF_1"
        );
        assert_eq!(
            select_issue_field(&fields, None, Some("IF_1"))
                .unwrap()
                .name,
            "Priority"
        );
    }

    #[test]
    fn coerces_all_supported_field_types() {
        assert_eq!(
            coerce_field_value(&field("IssueFieldText"), "hello").unwrap(),
            serde_json::json!({"fieldId": "IF_1", "textValue": "hello"})
        );
        assert_eq!(
            coerce_field_value(&field("IssueFieldNumber"), "42.5").unwrap(),
            serde_json::json!({"fieldId": "IF_1", "numberValue": 42.5})
        );
        assert_eq!(
            coerce_field_value(&field("IssueFieldDate"), "2030-01-02").unwrap(),
            serde_json::json!({"fieldId": "IF_1", "dateValue": "2030-01-02"})
        );
        assert_eq!(
            coerce_field_value(&field("IssueFieldSingleSelect"), "high").unwrap(),
            serde_json::json!({
                "fieldId": "IF_1",
                "singleSelectOptionId": "OPT_HIGH"
            })
        );
    }

    #[test]
    fn coercion_reports_invalid_and_unsupported_values() {
        assert!(
            coerce_field_value(&field("IssueFieldNumber"), "many")
                .unwrap_err()
                .contains("valid number")
        );
        assert!(
            coerce_field_value(&field("IssueFieldDate"), "2030-02-30")
                .unwrap_err()
                .contains("YYYY-MM-DD")
        );
        assert!(
            coerce_field_value(&field("IssueFieldSingleSelect"), "Urgent")
                .unwrap_err()
                .contains("allowed options")
        );
        assert!(
            coerce_field_value(&field("IssueFieldMultiSelect"), "High")
                .unwrap_err()
                .contains("unsupported type")
        );
    }

    #[tokio::test]
    async fn executes_discovery_coercion_and_mutation() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "number": 7,
                "node_id": "ISSUE_7",
                "title": "Allowed issue",
                "state": "open",
                "labels": [{"name": "automation"}],
                "html_url": "https://github.example/octo/repo/issues/7"
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_json(serde_json::json!({
                "query": DISCOVER_ISSUE_FIELDS,
                "variables": {"owner": "octo", "repo": "repo"}
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {
                    "repository": {
                        "issueFields": {
                            "nodes": [{
                                "__typename": "IssueFieldSingleSelect",
                                "id": "IF_1",
                                "name": "Priority",
                                "options": [{"id": "OPT_HIGH", "name": "High"}]
                            }]
                        }
                    }
                }
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_json(serde_json::json!({
                "query": SET_ISSUE_FIELD_VALUE,
                "variables": {
                    "issueId": "ISSUE_7",
                    "issueFields": [{
                        "fieldId": "IF_1",
                        "singleSelectOptionId": "OPT_HIGH"
                    }]
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {"setIssueFieldValue": {"issue": {"id": "ISSUE_7", "number": 7}}}
            })))
            .expect(1)
            .mount(&server)
            .await;

        let mut tool_configs = HashMap::new();
        tool_configs.insert(
            "set-github-issue-field".to_string(),
            serde_json::json!({
                "target-repo": "octo/repo",
                "required-labels": ["automation"],
                "required-title-prefix": "Allowed",
                "allowed-fields": ["*"]
            }),
        );
        let ctx = ExecutionContext {
            github_token: Some("token".to_string()),
            github_api_url: server.uri(),
            tool_configs,
            ..Default::default()
        };
        let mut result: SetGithubIssueFieldResult = SetGithubIssueFieldParams {
            issue_number: GithubIssueNumber::Number(7),
            field_name: Some("priority".to_string()),
            field_node_id: None,
            value: "high".to_string(),
            repository: None,
        }
        .try_into()
        .unwrap();
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
                .and_then(|data| data["field_name"].as_str()),
            Some("Priority")
        );
    }

    #[tokio::test]
    async fn unsupported_discovery_is_an_explicit_failure() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "number": 7,
                "node_id": "ISSUE_7",
                "title": "Issue",
                "state": "open",
                "labels": []
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": null,
                "errors": [{"type": "undefinedField", "message": "Field 'issueFields' doesn't exist"}]
            })))
            .mount(&server)
            .await;
        let mut tool_configs = HashMap::new();
        tool_configs.insert(
            "set-github-issue-field".to_string(),
            serde_json::json!({
                "target-repo": "octo/repo",
                "allowed-fields": ["Priority"]
            }),
        );
        let ctx = ExecutionContext {
            github_token: Some("token".to_string()),
            github_api_url: server.uri(),
            tool_configs,
            ..Default::default()
        };
        let mut result: SetGithubIssueFieldResult = SetGithubIssueFieldParams {
            issue_number: GithubIssueNumber::Number(7),
            field_name: Some("Priority".to_string()),
            field_node_id: None,
            value: "High".to_string(),
            repository: None,
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("unsupported or unavailable"));
    }
}
