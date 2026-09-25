use anyhow::Result;
use serde_yaml::{Mapping, Value};

use super::{Codemod, CodemodContext};

/// Keep in sync with the release introducing triggering-only defaults.
pub(crate) const INTRODUCED_IN: &str = "0.53.0";

pub static CODEMOD: Codemod = Codemod {
    id: "explicit_pr_policy",
    summary: "pin PR target defaults; remove sync-stack, which never synchronized Azure DevOps branches",
    introduced_in: INTRODUCED_IN,
    apply,
};

fn apply(front_matter: &mut Mapping, ctx: &CodemodContext) -> Result<bool> {
    if !crate::safe_outputs::pr_common::PR_MUTATION_TOOLS
        .iter()
        .any(|tool| {
            front_matter
                .get("safe-outputs")
                .and_then(|outputs| outputs.get(*tool))
                .is_some()
        })
    {
        return Ok(false);
    }
    let custom = crate::compile::imports::pr_policy::local_custom_job_names(front_matter)?;
    let Some(Value::Mapping(outputs)) = front_matter.get_mut("safe-outputs") else {
        return Ok(false);
    };
    let old = ctx
        .source_compiler_version
        .as_deref()
        .is_some_and(|version| crate::version::is_older_than(version, INTRODUCED_IN));
    let mut changed = false;
    for tool in crate::safe_outputs::pr_common::PR_MUTATION_TOOLS {
        if custom.contains(*tool) {
            continue;
        }
        let Some(config) = outputs.get_mut(*tool) else {
            continue;
        };
        if config.is_null() {
            *config = Value::Mapping(Mapping::new());
        }
        let Some(config) = config.as_mapping_mut() else {
            continue;
        };
        if !config.contains_key("target") {
            let was_explicit = matches!(
                *tool,
                "add-pull-request-reviewers"
                    | "add-pull-request-labels"
                    | "set-pull-request-auto-complete"
                    | "submit-pull-request-review"
                    | "add-pull-request-comment"
                    | "reply-to-pull-request-comment"
                    | "resolve-pull-request-thread"
            );
            config.insert(
                Value::String("target".into()),
                Value::String(
                    if old && was_explicit {
                        "*"
                    } else {
                        "triggering"
                    }
                    .into(),
                ),
            );
            changed = true;
        }
        if *tool == "update-pull-request" && config.remove("sync-stack").is_some() {
            changed = true;
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_pr_policy_preserves_old_scope_and_pins_new_defaults_idempotently() {
        for (version, expected) in [
            (None, "triggering"),
            (Some("0.52.1"), "*"),
            (Some("0.53.0"), "triggering"),
        ] {
            let mut mapping: Mapping = serde_yaml::from_str(
                "safe-outputs:\n  add-pull-request-comment: {}\n  update-pull-request:\n    sync-stack: true\n"
            ).unwrap();
            let ctx = CodemodContext::for_source(version.map(str::to_string));
            assert!(apply(&mut mapping, &ctx).unwrap());
            assert_eq!(
                mapping["safe-outputs"]["add-pull-request-comment"]["target"].as_str(),
                Some(expected)
            );
            assert_eq!(
                mapping["safe-outputs"]["update-pull-request"]["target"].as_str(),
                Some("triggering")
            );
            assert!(
                mapping["safe-outputs"]["update-pull-request"]
                    .get("sync-stack")
                    .is_none()
            );
            assert!(
                !apply(
                    &mut mapping,
                    &CodemodContext::for_source(Some("0.52.1".into()))
                )
                .unwrap()
            );
        }
    }

    #[test]
    fn explicit_pr_policy_does_not_override_declared_target() {
        let mut mapping: Mapping = serde_yaml::from_str(
            "safe-outputs:\n  submit-pull-request-review:\n    target: '42'\n    allowed-events: [comment]\n"
        ).unwrap();
        assert!(
            !apply(
                &mut mapping,
                &CodemodContext::for_source(Some("0.52.1".into()))
            )
            .unwrap()
        );
        assert_eq!(
            mapping["safe-outputs"]["submit-pull-request-review"]["target"].as_str(),
            Some("42")
        );
    }
}
