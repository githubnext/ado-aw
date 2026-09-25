//! Shared PR reference, target and trusted migration-policy handling.

use anyhow::{Context, ensure};
use percent_encoding::utf8_percent_encode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::result::AdoRepositoryTarget;
use super::update_pr::UpdatePrConfig;
use super::{
    ExecutionContext, ExecutionResult, PATH_SEGMENT, canonical_repository_alias,
    resolve_repository_write_target,
};
use crate::sanitize::SanitizeConfig;
use crate::secure::PullRequestTemporaryId;

pub(crate) const MAX_DESCRIPTION_UTF16: usize = 4_000;

pub(crate) async fn fetch_pr_labels(
    client: &reqwest::Client,
    base_url: &str,
    pr_id: u64,
    token: &str,
    ctx: &ExecutionContext,
) -> anyhow::Result<Result<Vec<String>, ExecutionResult>> {
    #[derive(Deserialize)]
    struct Label {
        name: String,
    }
    #[derive(Deserialize)]
    struct Labels {
        value: Vec<Label>,
    }

    // The general PR endpoint can omit labels even when the PR has them.
    let url = format!("{base_url}/pullRequests/{pr_id}/labels?api-version=7.1");
    let response = super::authenticate_ado_request(
        client.get(url),
        token,
        ctx.write_connection_type,
    )
    .send()
    .await
    .context("Failed to fetch pull request labels")?;
    if !response.status().is_success() {
        return Ok(Err(ExecutionResult::failure(format!(
            "Failed to fetch PR #{pr_id} labels (HTTP {})",
            response.status()
        ))));
    }
    match response.json::<Labels>().await {
        Ok(labels) => Ok(Ok(labels.value.into_iter().map(|label| label.name).collect())),
        Err(error) => Ok(Err(ExecutionResult::failure(format!(
            "Failed to parse PR #{pr_id} labels: {error}"
        )))),
    }
}

/// Normalized executor/preview target contract. Decimal strings avoid JS precision loss.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum PrTargetPolicy {
    Triggering,
    Explicit,
    Fixed { id: String },
}

impl PrTargetPolicy {
    pub(crate) fn named(value: &str) -> anyhow::Result<Self> {
        match value {
            "triggering" => Ok(Self::Triggering),
            "*" => Ok(Self::Explicit),
            value => Self::fixed(
                value
                    .parse::<u64>()
                    .context("target must be triggering, *, or a positive PR ID")?,
            ),
        }
    }

    pub(crate) fn fixed(id: u64) -> anyhow::Result<Self> {
        ensure!(id > 0, "target PR ID must be positive");
        Ok(Self::Fixed { id: id.to_string() })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggeringPullRequest {
    pub collection_uri: String,
    pub project: String,
    pub repository_name: String,
    pub repository_id: String,
    /// Kept as a validated decimal string across the compiler/Node/Rust boundary.
    pub id: String,
}

fn env_value(env: &impl Fn(&str) -> Option<String>, key: &str) -> Option<String> {
    env(key).filter(|value| !value.is_empty() && !value.contains("$(") && !value.contains("$["))
}

/// Exact collection identity; known Services URL spellings normalize to one canonical URL.
fn collection_identity(raw: &str) -> Option<String> {
    let url = url::Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "https" | "http")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let host = url.host_str()?.to_ascii_lowercase();
    let parts: Vec<_> = url
        .path_segments()?
        .filter(|part| !part.is_empty())
        .collect();
    if url.scheme() == "https" && url.port().is_none() {
        if host == "dev.azure.com" && parts.len() == 1 {
            return Some(format!(
                "https://dev.azure.com/{}",
                parts[0].to_ascii_lowercase()
            ));
        }
        if let Some(org) = host.strip_suffix(".visualstudio.com")
            && !org.is_empty()
            && !org.contains('.')
            && (parts.is_empty()
                || (parts.len() == 1 && parts[0].eq_ignore_ascii_case("DefaultCollection")))
        {
            return Some(format!("https://dev.azure.com/{org}"));
        }
        if host == "dev.azure.com" || host.ends_with(".visualstudio.com") {
            return None;
        }
    }
    // Exact non-Services collection identities are useful for already-trusted
    // contexts; native capture below only accepts supported Azure Repos remotes.
    Some(url.as_str().trim_end_matches('/').to_ascii_lowercase())
}

impl TriggeringPullRequest {
    fn validate(&self) -> anyhow::Result<u64> {
        ensure!(
            collection_identity(&self.collection_uri).is_some(),
            "invalid triggering collection URI"
        );
        crate::secure::Guid::parse(&self.repository_id)?;
        ensure!(
            !self.project.trim().is_empty() && !self.repository_name.trim().is_empty(),
            "incomplete triggering repository identity"
        );
        ensure!(
            !self.project.contains(['/', '\\'])
                && !self.repository_name.contains(['/', '\\'])
                && !self.project.chars().any(char::is_control)
                && !self.repository_name.chars().any(char::is_control),
            "invalid triggering repository path segment"
        );
        crate::validate::reject_pipeline_injection(&self.project, "triggering project")?;
        crate::validate::reject_pipeline_injection(&self.repository_name, "triggering repository")?;
        ensure!(
            !self.id.is_empty() && self.id.bytes().all(|byte| byte.is_ascii_digit()),
            "invalid triggering PR ID"
        );
        let id = self.id.parse::<u64>()?;
        ensure!(id > 0, "invalid triggering PR ID");
        Ok(id)
    }

    pub(crate) fn from_env(env: &impl Fn(&str) -> Option<String>) -> Option<Self> {
        // A projected synthetic tuple is authoritative. An empty/malformed tuple
        // must never fall back to unrelated native/self metadata.
        if let Some(raw) = env("ADO_AW_TRIGGERING_PR_IDENTITY") {
            let identity: Self = serde_json::from_str(&raw).ok()?;
            identity.validate().ok()?;
            if !collection_identity(&identity.collection_uri)?.starts_with("https://dev.azure.com/")
            {
                return None;
            }
            return Some(identity);
        }
        let get = |projected: &str, native: &str| {
            if env("ADO_AW_TRIGGERING_PR_CAPTURED").is_some() {
                env_value(env, projected)
            } else {
                env_value(env, native)
            }
        };
        if get("ADO_AW_TRIGGER_BUILD_REASON", "BUILD_REASON")? != "PullRequest"
            || get(
                "ADO_AW_TRIGGER_REPOSITORY_PROVIDER",
                "BUILD_REPOSITORY_PROVIDER",
            )? != "TfsGit"
        {
            return None;
        }
        let repository_uri = get("ADO_AW_TRIGGER_REPOSITORY_URI", "BUILD_REPOSITORY_URI")?;
        let remote = url::Url::parse(&repository_uri).ok()?;
        if remote.scheme() != "https"
            || remote.port().is_some()
            || remote.query().is_some()
            || remote.fragment().is_some()
            || remote.password().is_some()
        {
            return None;
        }
        let host = remote.host_str()?;
        if host != "dev.azure.com"
            && !host
                .strip_suffix(".visualstudio.com")
                .is_some_and(|org| !org.is_empty() && !org.contains('.'))
        {
            return None;
        }
        let mut segments: Vec<_> = remote.path_segments()?.collect();
        if segments.last() == Some(&"") {
            segments.pop();
        }
        if segments.iter().any(|part| part.is_empty()) {
            return None;
        }
        let expected_len = if host == "dev.azure.com"
            || segments
                .first()
                .is_some_and(|part| part.eq_ignore_ascii_case("DefaultCollection"))
        {
            4
        } else {
            3
        };
        if segments.len() != expected_len || segments[expected_len - 2] != "_git" {
            return None;
        }
        let parsed = crate::ado::parse_ado_remote(remote.as_str()).ok()?;
        let decode = |part: &str| {
            percent_encoding::percent_decode_str(part)
                .decode_utf8()
                .ok()
                .map(|part| part.into_owned())
        };
        let collection_uri =
            get("ADO_AW_TRIGGER_COLLECTION_URI", "SYSTEM_COLLECTIONURI").or_else(|| {
                env("ADO_AW_TRIGGERING_PR_CAPTURED")
                    .is_none()
                    .then(|| env_value(env, "SYSTEM_TEAMFOUNDATIONCOLLECTIONURI"))
                    .flatten()
            })?;
        if collection_identity(&collection_uri)? != collection_identity(&parsed.org_url)? {
            return None;
        }
        let identity = Self {
            collection_uri,
            project: decode(&parsed.project)?,
            // Do not use parse_ado_remote's git-suffix stripping for repository names.
            repository_name: decode(segments[expected_len - 1])?,
            repository_id: get("ADO_AW_TRIGGER_REPOSITORY_ID", "BUILD_REPOSITORY_ID")?,
            id: get("ADO_AW_TRIGGER_PR_ID", "SYSTEM_PULLREQUEST_PULLREQUESTID")?,
        };
        identity.validate().ok()?;
        Some(identity)
    }

    fn matches(&self, target: &AdoRepositoryTarget) -> bool {
        collection_identity(&self.collection_uri) == collection_identity(&target.organization_url)
            && self.project.eq_ignore_ascii_case(&target.project)
            && match &target.repository_id {
                Some(id) => id.eq_ignore_ascii_case(&self.repository_id),
                None => {
                    target
                        .repository
                        .eq_ignore_ascii_case(&self.repository_name)
                        || target.repository.eq_ignore_ascii_case(&self.repository_id)
                }
            }
    }
}

pub(crate) fn resolve_pr_policy_target(
    policy: &PrTargetPolicy,
    reference: Option<&PullRequestReference>,
    repository: Option<&str>,
    allowed: &[String],
    ctx: &ExecutionContext,
) -> anyhow::Result<Result<(u64, AdoRepositoryTarget), ExecutionResult>> {
    let failure = |message: String| Ok(Err(ExecutionResult::failure(message)));
    let (expected, triggering) = match policy {
        PrTargetPolicy::Explicit => (None, None),
        PrTargetPolicy::Fixed { id } => (Some(id.parse::<u64>()?), None),
        PrTargetPolicy::Triggering => {
            let Some(identity) = &ctx.triggering_pr else {
                return failure("target 'triggering' requires a complete trusted Azure DevOps triggering PR identity".into());
            };
            let id = match identity.validate() {
                Ok(id) => id,
                Err(error) => return failure(error.to_string()),
            };
            (Some(id), Some(identity))
        }
    };
    let default_reference;
    let reference = match reference {
        Some(reference) => reference,
        None => {
            let Some(id) = expected else {
                return failure("pull_request_id is required when target is '*'".into());
            };
            default_reference = PullRequestReference::Number(id);
            &default_reference
        }
    };
    let mapped_alias;
    let repository = if let Some(identity) = triggering
        && repository.is_none()
        && matches!(reference, PullRequestReference::Number(_))
    {
        let mut aliases: Vec<_> = ctx
            .allowed_repositories
            .keys()
            .chain(ctx.repository_targets.keys())
            .cloned()
            .collect();
        aliases.push("self".into());
        aliases.sort();
        aliases.dedup();
        mapped_alias = aliases.into_iter().find(|alias| {
            repository_is_allowed(allowed, alias, ctx)
                && resolve_repository_write_target(Some(alias), ctx)
                    .is_ok_and(|target| identity.matches(&target))
        });
        let Some(alias) = mapped_alias.as_deref() else {
            return failure(
                "triggering repository cannot be mapped to a permitted checkout/write target within allowed-repositories"
                    .into(),
            );
        };
        Some(alias)
    } else {
        repository
    };
    let (id, mut target) = match resolve_pr_target(reference, repository, allowed, ctx)? {
        Ok(target) => target,
        Err(failure) => return Ok(Err(failure)),
    };
    if let Some(expected) = expected
        && id != expected
    {
        return failure(format!(
            "requested pull_request_id #{id} does not match configured target #{expected}"
        ));
    }
    if let Some(identity) = triggering {
        if !identity.matches(&target) {
            return failure("requested PR destination does not match the trusted triggering collection/project/repository".into());
        }
        // A registry reference must still map to a permitted checkout in this context.
        let permitted = resolve_repository_write_target(Some(&target.alias), ctx)
            .is_ok_and(|permitted| identity.matches(&permitted));
        if !permitted {
            return failure(
                "triggering repository cannot be mapped to a permitted checkout/write target within allowed-repositories"
                    .into(),
            );
        }
        target.repository_id = Some(identity.repository_id.clone());
    }
    Ok(Ok((id, target)))
}

/// Positive Azure DevOps pull-request ID or a same-run temporary ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum PullRequestReference {
    Number(u64),
    Temporary(PullRequestTemporaryId),
}

impl std::fmt::Display for PullRequestReference {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Number(id) => write!(formatter, "{id}"),
            Self::Temporary(id) => formatter.write_str(&id.canonical()),
        }
    }
}

impl<'de> Deserialize<'de> for PullRequestReference {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ReferenceVisitor;
        impl serde::de::Visitor<'_> for ReferenceVisitor {
            type Value = PullRequestReference;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a positive pull-request ID or #aw_ temporary ID")
            }
            fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(PullRequestReference::Number(value))
            }
            fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Self::Value, E> {
                u64::try_from(value)
                    .map(PullRequestReference::Number)
                    .map_err(|_| E::custom("pull_request_id must be positive"))
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                let value = value.trim();
                let numeric = value.strip_prefix('#').unwrap_or(value);
                if !numeric.is_empty() && numeric.bytes().all(|c| c.is_ascii_digit()) {
                    return numeric
                        .parse::<u64>()
                        .map(PullRequestReference::Number)
                        .map_err(|_| E::custom("quoted pull_request_id is outside the u64 range"));
                }
                PullRequestTemporaryId::parse(value)
                    .map(PullRequestReference::Temporary)
                    .map_err(E::custom)
            }
        }
        deserializer.deserialize_any(ReferenceVisitor)
    }
}

pub(crate) fn validate_reference(reference: &PullRequestReference) -> anyhow::Result<()> {
    if let PullRequestReference::Number(id) = reference {
        ensure!(*id > 0, "pull_request_id must be a positive integer");
    }
    Ok(())
}

pub(crate) fn repository_api_base(target: &AdoRepositoryTarget) -> String {
    format!(
        "{}/{}/_apis/git/repositories/{}",
        target.organization_url.trim_end_matches('/'),
        utf8_percent_encode(&target.project, PATH_SEGMENT),
        utf8_percent_encode(target.repository_locator(), PATH_SEGMENT)
    )
}

fn repository_is_allowed(allowed: &[String], alias: &str, ctx: &ExecutionContext) -> bool {
    allowed.is_empty()
        || allowed.iter().any(|allowed| {
            allowed.eq_ignore_ascii_case(alias)
                || canonical_repository_alias(allowed, ctx)
                    .is_some_and(|candidate| candidate.eq_ignore_ascii_case(alias))
        })
}

pub(crate) fn resolve_pr_target(
    reference: &PullRequestReference,
    requested_repository: Option<&str>,
    allowed_repositories: &[String],
    ctx: &ExecutionContext,
) -> anyhow::Result<Result<(u64, AdoRepositoryTarget), ExecutionResult>> {
    if requested_repository.is_some_and(|selector| selector.trim().is_empty()) {
        return Ok(Err(ExecutionResult::failure(
            "explicit repository selector must not be empty",
        )));
    }
    if let Err(error) = validate_reference(reference) {
        return Ok(Err(ExecutionResult::failure(error.to_string())));
    }
    let (id, target) = match reference {
        PullRequestReference::Number(id) => {
            let target = match resolve_repository_write_target(requested_repository, ctx) {
                Ok(target) => target,
                Err(failure) => return Ok(Err(failure)),
            };
            (*id, target)
        }
        PullRequestReference::Temporary(temporary_id) => {
            let Some(resolved) = ctx.resolve_pull_request(temporary_id)? else {
                return Ok(Err(ExecutionResult::failure(format!(
                    "temporary pull-request ID '{}' has not been resolved; create-pull-request must succeed earlier in the same SafeOutputs job",
                    temporary_id.canonical()
                ))));
            };
            // The producer's exact destination is authoritative. Never reconstruct it from
            // the consumer's default repository/current project when the selector is absent.
            if let Some(selector) = requested_repository {
                let alias = canonical_repository_alias(selector, ctx);
                if alias
                    .as_deref()
                    .is_some_and(|alias| !alias.eq_ignore_ascii_case(&resolved.target.alias))
                {
                    return Ok(Err(ExecutionResult::failure(format!(
                        "temporary pull-request ID '{}' resolved to repository '{}', which does not match requested repository '{}'",
                        temporary_id.canonical(),
                        resolved.target.alias,
                        crate::sanitize::neutralize_pipeline_commands(selector)
                    ))));
                }
                let requested = match resolve_repository_write_target(Some(selector), ctx) {
                    Ok(target) => target,
                    Err(failure) => return Ok(Err(failure)),
                };
                if !requested.alias.eq_ignore_ascii_case(&resolved.target.alias)
                    || !requested
                        .organization_url
                        .trim_end_matches('/')
                        .eq_ignore_ascii_case(
                            resolved.target.organization_url.trim_end_matches('/'),
                        )
                    || !requested
                        .project
                        .eq_ignore_ascii_case(&resolved.target.project)
                    || !requested
                        .repository
                        .eq_ignore_ascii_case(&resolved.target.repository)
                {
                    return Ok(Err(ExecutionResult::failure(format!(
                        "temporary pull-request ID '{}' resolved to repository '{}', which does not match requested repository '{}'",
                        temporary_id.canonical(),
                        resolved.target.alias,
                        crate::sanitize::neutralize_pipeline_commands(selector)
                    ))));
                }
            }
            if resolved.id == 0 {
                return Ok(Err(ExecutionResult::failure(
                    "resolved pull_request_id must be positive",
                )));
            }
            (resolved.id, resolved.target)
        }
    };
    if !repository_is_allowed(allowed_repositories, &target.alias, ctx) {
        return Ok(Err(ExecutionResult::failure(format!(
            "Repository '{}' is not in the allowed-repositories list: [{}]",
            target.alias,
            allowed_repositories.join(", ")
        ))));
    }
    Ok(Ok((id, target)))
}

/// Read compatibility policy only from trusted execution configuration, never proposal JSON.
pub(crate) fn legacy_policy(
    ctx: &ExecutionContext,
    tool: &str,
    operation: &str,
) -> anyhow::Result<Option<UpdatePrConfig>> {
    let Some(value) = ctx
        .tool_configs
        .get(tool)
        .and_then(|config| config.get("legacy-update-pr"))
    else {
        return Ok(None);
    };
    ensure!(
        value.is_object(),
        "{tool}.legacy-update-pr must be an object"
    );
    let mut config: UpdatePrConfig =
        serde_json::from_value(value.clone()).context("invalid legacy-update-pr policy")?;
    crate::compile::pr_migration::validate_legacy_votes(&config.allowed_votes)?;
    config.sanitize_config_fields();
    ensure!(
        config.allowed_operations.is_empty()
            || config
                .allowed_operations
                .iter()
                .any(|allowed| allowed == operation),
        "Operation '{operation}' is not in the legacy allowed-operations list"
    );
    Ok(Some(config))
}

pub(crate) fn validate_description(body: &str) -> anyhow::Result<()> {
    ensure!(
        body.encode_utf16().count() <= MAX_DESCRIPTION_UTF16,
        "updated body exceeds Azure DevOps' {MAX_DESCRIPTION_UTF16}-UTF-16-unit limit"
    );
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::safe_outputs::{AbandonPullRequestResult, Executor, UpdatePullRequestResult};

    #[test]
    fn pr_tool_schemas_and_deserializers_are_closed() {
        fn check<T: JsonSchema + serde::de::DeserializeOwned>() {
            let schema = serde_json::to_value(schemars::schema_for!(T)).unwrap();
            assert_eq!(
                schema["additionalProperties"], false,
                "{} must reject unknown properties", std::any::type_name::<T>(),
            );
            let error = serde_json::from_value::<T>(
                serde_json::json!({"unsupported-policy": true}),
            )
            .err()
            .expect("unknown property must be rejected");
            assert!(
                error.to_string().contains("unknown field `unsupported-policy`"),
                "{}: {error}", std::any::type_name::<T>(),
            );
        }
        use crate::safe_outputs::*;
        check::<CreatePrParams>();
        check::<CreatePrResult>();
        check::<AddPrCommentParams>();
        check::<AddPrCommentResult>();
        check::<ReplyToPrCommentParams>();
        check::<ReplyToPrCommentResult>();
        check::<ResolvePrThreadParams>();
        check::<ResolvePrThreadResult>();
        check::<SubmitPrReviewParams>();
        check::<SubmitPrReviewResult>();
        check::<AddPrReviewersParams>();
        check::<AddPrReviewersResult>();
        check::<AddPrLabelsParams>();
        check::<AddPrLabelsResult>();
        check::<SetPrAutoCompleteParams>();
        check::<SetPrAutoCompleteResult>();
        check::<UpdatePullRequestParams>();
        check::<UpdatePullRequestResult>();
        check::<AbandonPullRequestParams>();
        check::<AbandonPullRequestResult>();
    }

    const TRIGGER_REPO_ID: &str = "11111111-1111-1111-1111-111111111111";

    fn native_env() -> std::collections::HashMap<String, String> {
        [
            ("BUILD_REASON", "PullRequest"),
            ("BUILD_REPOSITORY_PROVIDER", "TfsGit"),
            ("SYSTEM_COLLECTIONURI", "https://dev.azure.com/org/"),
            (
                "BUILD_REPOSITORY_URI",
                "https://dev.azure.com/org/Other/_git/target",
            ),
            ("BUILD_REPOSITORY_ID", TRIGGER_REPO_ID),
            ("SYSTEM_PULLREQUEST_PULLREQUESTID", "42"),
            ("ADO_AW_SELF_REPOSITORY_NAME", "templates"),
            ("SYSTEM_TEAMPROJECT", "Current"),
            (
                "SYSTEM_PULLREQUEST_SOURCEREPOSITORYURI",
                "https://dev.azure.com/fork/Other/_git/target",
            ),
        ]
        .into_iter()
        .map(|(key, value)| (key.into(), value.into()))
        .collect()
    }

    #[test]
    fn native_capture_is_independent_of_self_cli_and_fork_source() {
        let mut env = native_env();
        env.insert(
            "AZURE_DEVOPS_ORG_URL".into(),
            "https://dev.azure.com/overridden".into(),
        );
        let mut ctx = ExecutionContext::from_env_lookup(|key| env.get(key).cloned());
        let identity = ctx.triggering_pr.clone().unwrap();
        assert_eq!(identity.collection_uri, "https://dev.azure.com/org/");
        assert_eq!(identity.project, "Other");
        assert_eq!(identity.repository_name, "target");
        assert_eq!(identity.repository_id, TRIGGER_REPO_ID);
        assert_eq!(ctx.repository_name.as_deref(), Some("templates"));
        ctx.ado_org_url = Some("https://dev.azure.com/cli".into());
        ctx.ado_project = Some("cli-project".into());
        assert_eq!(ctx.triggering_pr, Some(identity));
    }

    #[test]
    fn native_capture_normalizes_supported_url_spellings_without_relaxing_paths() {
        let mut env = native_env();
        env.insert(
            "BUILD_REPOSITORY_URI".into(),
            "https://DEV.AZURE.COM/org/Other/_git/target/".into(),
        );
        env.insert(
            "SYSTEM_COLLECTIONURI".into(),
            "https://org.visualstudio.com/DefaultCollection/".into(),
        );
        env.insert(
            "SYSTEM_PULLREQUEST_PULLREQUESTID".into(),
            u64::MAX.to_string(),
        );
        assert_eq!(
            TriggeringPullRequest::from_env(&|key| env.get(key).cloned())
                .unwrap()
                .id,
            u64::MAX.to_string()
        );
        for uri in [
            "https://dev.azure.com/org/extra",
            "https://org.visualstudio.com/OtherCollection/",
        ] {
            env.insert("SYSTEM_COLLECTIONURI".into(), uri.into());
            assert!(TriggeringPullRequest::from_env(&|key| env.get(key).cloned()).is_none());
        }
    }

    #[test]
    fn native_capture_rejects_missing_malformed_non_ado_and_unresolved_metadata() {
        for (key, value) in [
            ("BUILD_REASON", "Manual"),
            ("BUILD_REPOSITORY_PROVIDER", "GitHub"),
            ("BUILD_REPOSITORY_URI", ""),
            ("BUILD_REPOSITORY_ID", ""),
            ("SYSTEM_COLLECTIONURI", "https://dev.azure.com/wrong"),
            (
                "BUILD_REPOSITORY_URI",
                "https://dev.azure.com.evil/org/Other/_git/target",
            ),
            (
                "SYSTEM_PULLREQUEST_PULLREQUESTID",
                "$(System.PullRequest.PullRequestId)",
            ),
            ("SYSTEM_PULLREQUEST_PULLREQUESTID", "18446744073709551616"),
            (
                "BUILD_REPOSITORY_URI",
                "https://dev.azure.com/org/Other/_git/target/extra",
            ),
        ] {
            let mut env = native_env();
            env.insert(key.into(), value.into());
            assert!(
                ExecutionContext::from_env_lookup(|key| env.get(key).cloned())
                    .triggering_pr
                    .is_none(),
                "{key}"
            );
        }
        let mut env = native_env();
        env.insert("ADO_AW_TRIGGERING_PR_IDENTITY".into(), "".into());
        assert!(
            ExecutionContext::from_env_lookup(|key| env.get(key).cloned())
                .triggering_pr
                .is_none()
        );
        env.remove("ADO_AW_TRIGGERING_PR_IDENTITY");
        env.insert("ADO_AW_TRIGGERING_PR_CAPTURED".into(), "true".into());
        assert!(
            ExecutionContext::from_env_lookup(|key| env.get(key).cloned())
                .triggering_pr
                .is_none()
        );
    }

    async fn run_content_tool(
        tool: &str,
        ctx: &ExecutionContext,
        fields: serde_json::Value,
    ) -> ExecutionResult {
        let mut value = fields;
        value["name"] = serde_json::json!(tool);
        if tool == "update-pull-request" {
            value["title"] = serde_json::json!("Updated title");
            let mut proposal: UpdatePullRequestResult = serde_json::from_value(value).unwrap();
            proposal.execute_sanitized(ctx).await.unwrap()
        } else {
            let mut proposal: AbandonPullRequestResult = serde_json::from_value(value).unwrap();
            proposal.execute_sanitized(ctx).await.unwrap()
        }
    }

    fn triggering_context(
        server: &wiremock::MockServer,
        tool: &str,
        synthetic: bool,
    ) -> ExecutionContext {
        let mut env = native_env();
        if synthetic {
            let identity = TriggeringPullRequest::from_env(&|key| env.get(key).cloned()).unwrap();
            env.insert(
                "ADO_AW_TRIGGERING_PR_IDENTITY".into(),
                serde_json::to_string(&identity).unwrap(),
            );
            env.insert("BUILD_REASON".into(), "IndividualCI".into());
            env.remove("SYSTEM_PULLREQUEST_PULLREQUESTID");
        }
        let mut ctx = ExecutionContext::from_env_lookup(|key| env.get(key).cloned());
        // Replace transport origin only; the captured project/repository/PR tuple remains unchanged.
        ctx.ado_org_url = Some(server.uri());
        ctx.ado_organization = Some("org".into());
        ctx.triggering_pr.as_mut().unwrap().collection_uri = server.uri();
        ctx.access_token = Some("token".into());
        ctx.allowed_repositories
            .insert("trigger".into(), "Other/target".into());
        ctx.tool_configs.insert(tool.into(), serde_json::json!({}));
        ctx
    }

    #[tokio::test]
    async fn both_triggering_tools_use_permitted_native_and_synthetic_trigger_not_self() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };
        for tool in ["update-pull-request", "abandon-pull-request"] {
            for synthetic in [false, true] {
                let server = MockServer::start().await;
                let route =
                    format!("/Other/_apis/git/repositories/{TRIGGER_REPO_ID}/pullRequests/42");
                Mock::given(method("GET"))
                    .and(path(&route))
                    .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                        "pullRequestId": 42, "title": "Old title", "status": "active"
                    })))
                    .expect(4)
                    .mount(&server)
                    .await;
                Mock::given(method("PATCH"))
                    .and(path(&route))
                    .respond_with(ResponseTemplate::new(200))
                    .expect(4)
                    .mount(&server)
                    .await;
                let ctx = triggering_context(&server, tool, synthetic);
                ctx.register_resolved_pull_request(
                    &PullRequestTemporaryId::parse("#aw_known").unwrap(),
                    crate::safe_outputs::ResolvedPullRequest {
                        id: 42,
                        url: "unused".into(),
                        target: resolve_repository_write_target(Some("trigger"), &ctx).unwrap(),
                    },
                )
                .unwrap();
                for fields in [
                    serde_json::json!({}),
                    serde_json::json!({"pull_request_id":42}),
                    serde_json::json!({"pull_request_id":"42"}),
                    serde_json::json!({"pull_request_id":"#aw_known"}),
                ] {
                    let result = run_content_tool(tool, &ctx, fields).await;
                    assert!(
                        result.success,
                        "{tool} synthetic={synthetic}: {}",
                        result.message
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn same_id_foreign_org_project_repo_and_temp_refs_never_mutate() {
        use wiremock::MockServer;
        for tool in ["update-pull-request", "abandon-pull-request"] {
            for mismatch in ["org", "project", "repo", "guid"] {
                let server = MockServer::start().await;
                let ctx = triggering_context(&server, tool, false);
                let mut target = resolve_repository_write_target(Some("trigger"), &ctx).unwrap();
                match mismatch {
                    "org" => target.organization_url = "https://dev.azure.com/foreign".into(),
                    "project" => target.project = "Foreign".into(),
                    "repo" => target.repository = "foreign".into(),
                    _ => target.repository_id = Some("22222222-2222-2222-2222-222222222222".into()),
                }
                ctx.register_resolved_pull_request(
                    &PullRequestTemporaryId::parse("#aw_foreign").unwrap(),
                    crate::safe_outputs::ResolvedPullRequest {
                        id: 42,
                        url: "unused".into(),
                        target,
                    },
                )
                .unwrap();
                let result = run_content_tool(
                    tool,
                    &ctx,
                    serde_json::json!({"pull_request_id":"#aw_foreign"}),
                )
                .await;
                assert!(!result.success, "{tool} {mismatch}");
                assert!(server.received_requests().await.unwrap().is_empty());
            }
            let server = MockServer::start().await;
            let mut ctx = triggering_context(&server, tool, false);
            for fields in [
                serde_json::json!({"pull_request_id":42, "repository":"self"}),
                serde_json::json!({"pull_request_id":"42", "repository":""}),
                serde_json::json!({"pull_request_id":"#aw_unknown"}),
            ] {
                assert!(!run_content_tool(tool, &ctx, fields).await.success);
            }
            ctx.triggering_pr = None;
            assert!(
                !run_content_tool(tool, &ctx, serde_json::json!({"pull_request_id":42}))
                    .await
                    .success
            );
            assert!(server.received_requests().await.unwrap().is_empty());
        }
    }

    #[test]
    fn triggering_context_cannot_grant_checkout_or_cross_org_write_authority() {
        let mut env = native_env();
        let mut ctx = ExecutionContext::from_env_lookup(|key| env.get(key).cloned());
        assert!(
            resolve_pr_policy_target(&PrTargetPolicy::Triggering, None, None, &[], &ctx)
                .unwrap()
                .is_err()
        );
        ctx.allowed_repositories
            .insert("trigger".into(), "Other/target".into());
        assert!(
            resolve_pr_policy_target(
                &PrTargetPolicy::Triggering,
                None,
                None,
                &["self".into()],
                &ctx
            )
            .unwrap()
            .is_err()
        );
        env.insert(
            "SYSTEM_COLLECTIONURI".into(),
            "https://dev.azure.com/foreign/".into(),
        );
        env.insert(
            "BUILD_REPOSITORY_URI".into(),
            "https://dev.azure.com/foreign/Other/_git/target".into(),
        );
        ctx.triggering_pr = TriggeringPullRequest::from_env(&|key| env.get(key).cloned());
        assert!(
            resolve_pr_policy_target(&PrTargetPolicy::Triggering, None, None, &[], &ctx)
                .unwrap()
                .is_err()
        );
    }

    #[test]
    fn known_collection_spellings_and_repository_guids_share_trusted_identity() {
        let env = native_env();
        let identity = TriggeringPullRequest::from_env(&|key| env.get(key).cloned()).unwrap();
        let target = AdoRepositoryTarget {
            alias: "trigger".into(),
            organization: "org".into(),
            organization_url: "https://org.visualstudio.com/DefaultCollection/".into(),
            project: "Other".into(),
            repository: TRIGGER_REPO_ID.into(),
            repository_id: None,
            cross_organization: false,
        };
        assert!(identity.matches(&target));
        let mut wrong = target;
        wrong.organization_url = "https://org.visualstudio.com.evil/DefaultCollection".into();
        assert!(!identity.matches(&wrong));
    }

    pub(crate) fn registered_context(
        organization_url: &str,
        tool: &str,
        config: serde_json::Value,
    ) -> ExecutionContext {
        let mut ctx = ExecutionContext {
            ado_org_url: Some("https://dev.azure.com/current-org".into()),
            ado_organization: Some("current-org".into()),
            ado_project: Some("Current".into()),
            repository_name: Some("current".into()),
            access_token: Some("token".into()),
            ..Default::default()
        };
        ctx.tool_configs.insert(tool.into(), config);
        ctx.register_resolved_pull_request(
            &PullRequestTemporaryId::parse("#aw_pr123").unwrap(),
            super::super::ResolvedPullRequest {
                id: 4_294_967_296,
                url: format!("{organization_url}/pullrequest/4294967296"),
                target: AdoRepositoryTarget {
                    alias: "other".into(),
                    organization: "other-org".into(),
                    organization_url: organization_url.into(),
                    project: "Other".into(),
                    repository: "repo".into(),
                    repository_id: Some("repo-id".into()),
                    cross_organization: true,
                },
            },
        )
        .unwrap();
        ctx
    }

    #[test]
    fn reference_preserves_u64_and_compatibility_spellings() {
        for json in [
            "18446744073709551615",
            "\"18446744073709551615\"",
            "\" #18446744073709551615 \"",
        ] {
            assert_eq!(
                serde_json::from_str::<PullRequestReference>(json).unwrap(),
                PullRequestReference::Number(u64::MAX)
            );
        }
        assert!(serde_json::from_str::<PullRequestReference>("\"18446744073709551616\"").is_err());
        assert!(serde_json::from_str::<PullRequestReference>("-1").is_err());
    }

    #[test]
    fn description_limit_counts_utf16_not_bytes_or_scalars() {
        for size in [3999, 4000] {
            assert!(validate_description(&"a".repeat(size)).is_ok());
        }
        assert!(validate_description(&"a".repeat(4001)).is_err());
        assert!(validate_description(&"😀".repeat(2000)).is_ok());
        assert!(validate_description(&format!("{}a", "😀".repeat(2000))).is_err());
    }

    #[test]
    fn temporary_reference_checks_exact_target_not_only_alias() {
        let mut ctx = registered_context(
            "https://dev.azure.com/other-org",
            "add-pull-request-labels",
            serde_json::json!({}),
        );
        ctx.allowed_repositories
            .insert("other".into(), "Changed/repo".into());
        let reference: PullRequestReference = serde_json::from_str("\"#aw_pr123\"").unwrap();
        let (_, preserved) = resolve_pr_target(&reference, None, &[], &ctx)
            .unwrap()
            .unwrap();
        assert_eq!(preserved.project, "Other");
        assert_eq!(preserved.repository_locator(), "repo-id");
        let failure = resolve_pr_target(&reference, Some("other"), &[], &ctx)
            .unwrap()
            .unwrap_err();
        assert!(failure.message.contains("does not match"));
        assert!(
            resolve_pr_target(&reference, None, &["self".into()], &ctx)
                .unwrap()
                .is_err()
        );
    }

    #[test]
    fn legacy_metadata_is_trusted_config_only_and_fail_closed() {
        let mut ctx = ExecutionContext::default();
        ctx.tool_configs.insert(
            "add-pull-request-labels".into(),
            serde_json::json!({
                "legacy-update-pr": {"allowed-operations": ["vote"]}
            }),
        );
        assert!(legacy_policy(&ctx, "add-pull-request-labels", "add-labels").is_err());
        ctx.tool_configs.insert(
            "add-pull-request-labels".into(),
            serde_json::json!({
                "legacy-update-pr": null
            }),
        );
        assert!(legacy_policy(&ctx, "add-pull-request-labels", "add-labels").is_err());
        ctx.tool_configs
            .insert("add-pull-request-labels".into(), serde_json::json!({}));
        assert!(
            legacy_policy(&ctx, "add-pull-request-labels", "add-labels")
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn invalid_legacy_vote_metadata_never_enables_native_review_events() {
        use crate::safe_outputs::SubmitPrReviewResult;
        let server = wiremock::MockServer::start().await;
        for vote in ["comment", "request-changes", "unknown"] {
            let mut ctx = ExecutionContext {
                access_token: Some("token".into()),
                ado_org_url: Some(server.uri()),
                ado_organization: Some("org".into()),
                ado_project: Some("Other".into()),
                repository_name: Some("target".into()),
                ..Default::default()
            };
            ctx.tool_configs.insert(
                "submit-pull-request-review".into(),
                serde_json::json!({
                    "allowed-events": ["comment"], "legacy-update-pr": {"allowed-votes": [vote]}
                }),
            );
            let mut proposal: SubmitPrReviewResult = serde_json::from_value(serde_json::json!({
                "name":"submit-pull-request-review","pull_request_id":42,"event":"comment"
            }))
            .unwrap();
            let result = proposal.execute_sanitized(&ctx).await;
            assert!(
                result.is_err() || result.is_ok_and(|result| !result.success),
                "{vote}"
            );
        }
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}
