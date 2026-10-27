//! Shared ADO PR name-grant semantics, independent of repository write authority.

use super::Validate;
use anyhow::{Result, ensure};

pub(crate) fn validate_proposal(tool: &str, proposal: &serde_json::Value) -> Result<()> {
    let mut payload = proposal
        .as_object()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("PR proposal must be an object"))?;
    payload.remove("name");
    payload.remove("context");
    if tool == "create-pull-request" {
        for field in [
            "source_branch",
            "patch_file",
            "temporary_id",
            "base_commit",
            "patch_sha256",
        ] {
            payload.remove(field);
        }
        if let Some(labels) = payload.remove("agent_labels") {
            ensure!(
                !payload.contains_key("labels"),
                "Ambiguous creation label fields"
            );
            payload.insert("labels".into(), labels);
        }
    } else if tool == "push-to-pull-request-branch" {
        payload.remove("patch_file");
        payload.remove("patch_sha256");
    }
    let payload = serde_json::Value::Object(payload);
    macro_rules! validate {
        ($params:ty) => {
            serde_json::from_value::<$params>(payload)?.validate()
        };
    }
    match tool {
        "create-pull-request" => validate!(super::CreatePrParams),
        "update-pull-request" => validate!(super::UpdatePullRequestParams),
        "abandon-pull-request" => validate!(super::AbandonPullRequestParams),
        "add-pull-request-reviewers" => validate!(super::AddPrReviewersParams),
        "add-pull-request-labels" => validate!(super::AddPrLabelsParams),
        "remove-pull-request-labels" => validate!(super::RemovePullRequestLabelsParams),
        "replace-pull-request-label" => validate!(super::ReplacePullRequestLabelParams),
        "mark-pull-request-as-ready-for-review" => validate!(super::MarkPullRequestReadyParams),
        "set-pull-request-auto-complete" => validate!(super::SetPrAutoCompleteParams),
        "push-to-pull-request-branch" => validate!(super::PushToPullRequestBranchParams),
        "update-pull-request-comment" => validate!(super::UpdatePullRequestCommentParams),
        "add-pull-request-comment" => validate!(super::AddPrCommentParams),
        "reply-to-pull-request-comment" => validate!(super::ReplyToPrCommentParams),
        "resolve-pull-request-thread" => validate!(super::ResolvePrThreadParams),
        "submit-pull-request-review" => validate!(super::SubmitPrReviewParams),
        _ => anyhow::bail!("Unknown PR proposal tool '{tool}'"),
    }
}

pub(crate) fn unrestricted_names() -> Vec<String> {
    vec!["*".into()]
}

pub(crate) fn validate_names(names: &[String], field: &str) -> Result<()> {
    ensure!(
        !names.iter().any(|name| name.trim() == "*") || names.len() == 1,
        "{field} cannot mix '*' with specific names"
    );
    for name in names {
        ensure!(!name.trim().is_empty(), "{field} entries must not be blank");
        crate::validate::reject_pipeline_injection(name, field)?;
    }
    Ok(())
}

pub(crate) fn permits_name(names: &[String], requested: &str) -> bool {
    names
        .iter()
        .any(|name| name.trim() == "*" || name.trim().eq_ignore_ascii_case(requested.trim()))
}

pub(crate) fn comment_statuses() -> Vec<String> {
    vec!["active".into()]
}

pub(crate) fn resolution_statuses() -> Vec<String> {
    vec!["fixed".into()]
}

pub(crate) fn review_events() -> Vec<String> {
    vec!["comment".into()]
}

pub(crate) fn is_pr_tool(tool: &str) -> bool {
    tool == "create-pull-request" || super::pr_common::PR_MUTATION_TOOLS.contains(&tool)
}

pub(crate) fn has_usable_action(tool: &str, config: &serde_json::Value) -> bool {
    let empty = |field: &str| {
        config
            .get(field)
            .and_then(serde_json::Value::as_array)
            .is_some_and(Vec::is_empty)
    };
    let all_blocked = config
        .get("blocked-labels")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|names| {
            names
                .iter()
                .any(|name| name.as_str().is_some_and(|name| name.trim() == "*"))
        });
    if config.get("max").and_then(serde_json::Value::as_u64) == Some(0)
        || empty("allowed-repositories")
    {
        return false;
    }
    match tool {
        "add-pull-request-reviewers" => !empty("allowed-reviewers"),
        "add-pull-request-labels" | "remove-pull-request-labels" => {
            !empty("allowed-labels") && !all_blocked
        }
        "replace-pull-request-label" => {
            !empty("allowed-add")
                && !empty("allowed-remove")
                && !empty("allowed-transitions")
                && !all_blocked
        }
        "submit-pull-request-review" => !empty("allowed-events"),
        "add-pull-request-comment" | "resolve-pull-request-thread" => !empty("allowed-statuses"),
        "update-pull-request" => {
            config.get("title").and_then(serde_json::Value::as_bool) != Some(false)
                || (config.get("body").and_then(serde_json::Value::as_bool) != Some(false)
                    && !empty("allowed-operations"))
        }
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_grants_deny_and_wildcards_cannot_hide_specific_names() {
        assert!(!permits_name(&[], "label"));
        assert!(permits_name(&unrestricted_names(), "label"));
        assert!(permits_name(&["Label".into()], "label"));
        assert!(!permits_name(&["lab*".into()], "label"));
        assert!(validate_names(&["*".into(), "label".into()], "allowed-labels").is_err());
        assert!(validate_names(&["*".into()], "allowed-labels").is_ok());
        assert!(validate_names(&[], "allowed-labels").is_ok());
    }

    #[test]
    fn unusable_optional_capabilities_do_not_disable_other_actions() {
        assert!(!has_usable_action(
            "add-pull-request-labels",
            &serde_json::json!({"allowed-labels":[]})
        ));
        assert!(!has_usable_action(
            "submit-pull-request-review",
            &serde_json::json!({"allowed-events":[]})
        ));
        assert!(!has_usable_action(
            "update-pull-request",
            &serde_json::json!({
            "title":false,"allowed-operations":[]})
        ));
        assert!(has_usable_action(
            "update-pull-request",
            &serde_json::json!({"allowed-operations":[]})
        ));
        assert!(has_usable_action(
            "create-pull-request",
            &serde_json::json!({"allowed-labels":[]})
        ));
        assert!(has_usable_action(
            "submit-pull-request-review",
            &serde_json::json!({"max-comments":0})
        ));
        for tool in [
            "create-pull-request",
            "submit-pull-request-review",
            "add-pull-request-labels",
        ] {
            assert!(!has_usable_action(tool, &serde_json::json!({"max":0})));
            assert!(!has_usable_action(
                tool,
                &serde_json::json!({"allowed-repositories":[]})
            ));
        }
    }
}
