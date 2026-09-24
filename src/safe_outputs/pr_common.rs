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

pub(crate) fn resolved_reference_id(
    reference: &PullRequestReference,
    ctx: &ExecutionContext,
) -> Result<u64, ExecutionResult> {
    match reference {
        PullRequestReference::Number(id) if *id > 0 => Ok(*id),
        PullRequestReference::Number(_) => Err(ExecutionResult::failure(
            "pull_request_id must be positive",
        )),
        PullRequestReference::Temporary(id) => ctx
            .resolve_pull_request(id)
            .map_err(|error| ExecutionResult::failure(error.to_string()))?
            .filter(|resolved| resolved.id > 0)
            .map(|resolved| resolved.id)
            .ok_or_else(|| ExecutionResult::failure(format!(
                "temporary pull-request ID '{}' has not been resolved; create-pull-request must succeed earlier in the same SafeOutputs job",
                id.canonical()
            ))),
    }
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
}
