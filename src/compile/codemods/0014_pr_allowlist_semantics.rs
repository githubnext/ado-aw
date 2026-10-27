use anyhow::{Result, ensure};
use serde_yaml::{Mapping, Value};

use super::{Codemod, CodemodContext};

pub static CODEMOD: Codemod = Codemod {
    id: "pr_allowlist_semantics",
    summary: "make released PR name grants and action defaults explicit before adopting empty-means-deny policies",
    introduced_in: super::m0011_explicit_pr_policy::INTRODUCED_IN,
    apply,
};

pub(crate) fn preserve_legacy_grants(tool: &str, config: &mut Mapping) {
    if matches!(tool, "create-pull-request" | "add-pull-request-comment") {
        config
            .entry(Value::String("include-stats".into()))
            .or_insert(Value::Bool(true));
    }
    for field in [
        "allowed-repositories",
        "allowed-labels",
        "allowed-reviewers",
        "allowed-add",
        "allowed-remove",
    ] {
        let applicable = field == "allowed-repositories"
            || (field == "allowed-labels"
                && matches!(
                    tool,
                    "create-pull-request"
                        | "add-pull-request-labels"
                        | "remove-pull-request-labels"
                ))
            || (field == "allowed-reviewers" && tool == "add-pull-request-reviewers")
            || (matches!(field, "allowed-add" | "allowed-remove")
                && tool == "replace-pull-request-label");
        if applicable
            && config
                .get(field)
                .is_none_or(|value| value.as_sequence().is_some_and(Vec::is_empty))
        {
            config.insert(
                field.into(),
                serde_yaml::to_value(["*"]).expect("static list"),
            );
        }
    }
    if tool == "add-pull-request-comment"
        && config
            .get("allowed-statuses")
            .is_none_or(|value| value.as_sequence().is_some_and(Vec::is_empty))
    {
        config.insert(
            "allowed-statuses".into(),
            serde_yaml::to_value(["active", "fixed", "wont-fix", "closed", "by-design"])
                .expect("static list"),
        );
    }
    for (name, field) in [
        ("submit-pull-request-review", "allowed-events"),
        ("resolve-pull-request-thread", "allowed-statuses"),
    ] {
        if tool == name {
            config
                .entry(Value::String(field.into()))
                .or_insert(Value::Sequence(Vec::new()));
        }
    }
}

pub(crate) fn validate_legacy_literal_names(tool: &str, config: &Mapping) -> Result<()> {
    for field in [
        "allowed-repositories",
        "allowed-labels",
        "allowed-add",
        "allowed-remove",
        "blocked-labels",
    ] {
        if let Some(names) = config.get(field).and_then(Value::as_sequence) {
            ensure!(
                !names.iter().any(|name| name.as_str() == Some("*")),
                "manual migration required: {tool}.{field} contains a legacy literal '*'; explicitly choose a new name policy before upgrading, rather than changing its authority silently"
            );
        }
    }
    Ok(())
}

pub(crate) fn validate_legacy_source_names(
    front_matter: &Mapping,
    version: Option<&str>,
) -> Result<()> {
    let old = version
        .is_some_and(|version| crate::version::is_older_than(version, CODEMOD.introduced_in));
    let Some(outputs) = front_matter.get("safe-outputs").and_then(Value::as_mapping) else {
        return Ok(());
    };
    for tool in std::iter::once("create-pull-request").chain(
        crate::safe_outputs::pr_common::PR_MUTATION_TOOLS
            .iter()
            .copied(),
    ) {
        let Some(config) = outputs.get(tool).and_then(Value::as_mapping) else {
            continue;
        };
        if tool == "create-pull-request" && (old || config.contains_key("squash-merge")) {
            validate_legacy_literal_names(tool, config)?;
        }
    }
    Ok(())
}

fn apply(front_matter: &mut Mapping, ctx: &CodemodContext) -> Result<bool> {
    if !std::iter::once("create-pull-request")
        .chain(
            crate::safe_outputs::pr_common::PR_MUTATION_TOOLS
                .iter()
                .copied(),
        )
        .any(|tool| {
            front_matter
                .get("safe-outputs")
                .and_then(|outputs| outputs.get(tool))
                .is_some()
        })
    {
        return Ok(false);
    }
    let old = ctx
        .source_compiler_version
        .as_deref()
        .is_some_and(|version| crate::version::is_older_than(version, CODEMOD.introduced_in));
    let custom = crate::compile::imports::pr_policy::local_custom_job_names(front_matter)?;
    let mut migrated = front_matter.clone();
    let Some(Value::Mapping(outputs)) = migrated.get_mut("safe-outputs") else {
        return Ok(false);
    };
    for tool in std::iter::once("create-pull-request").chain(
        crate::safe_outputs::pr_common::PR_MUTATION_TOOLS
            .iter()
            .copied(),
    ) {
        if custom.contains(tool) {
            continue;
        }
        let Some(raw) = outputs.get_mut(tool) else {
            continue;
        };
        if old && tool == "create-pull-request" && raw.is_null() {
            *raw = Value::Mapping(Mapping::new());
        }
        let Some(config) = raw.as_mapping_mut() else {
            continue;
        };
        if old && tool == "create-pull-request" {
            preserve_legacy_grants(tool, config);
        }
    }
    let changed = migrated != *front_matter;
    if changed {
        *front_matter = migrated;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn released_empty_name_grants_are_preserved_but_new_empty_grants_are_not_widened() {
        let source: Mapping = serde_yaml::from_str(
            "safe-outputs:\n  create-pull-request: {allowed-repositories: [], allowed-labels: []}\n").unwrap();
        let mut old = source.clone();
        assert!(apply(&mut old, &CodemodContext::for_source(Some("0.53.0".into()))).unwrap());
        assert_eq!(
            old["safe-outputs"]["create-pull-request"]["allowed-repositories"][0],
            "*"
        );
        assert_eq!(
            old["safe-outputs"]["create-pull-request"]["allowed-labels"][0],
            "*"
        );
        let mut new = source.clone();
        assert!(!apply(&mut new, &CodemodContext::current()).unwrap());
        assert_eq!(new, source);
    }

    #[test]
    fn explicit_empty_grants_on_migrated_tools_are_not_rewidened() {
        let mut source: Mapping = serde_yaml::from_str(
            "safe-outputs:\n  add-pull-request-labels:\n    allowed-labels: []\n    allowed-repositories: []\n    legacy-update-pr:\n      allowed-operations: [add-labels]\n").unwrap();
        let before = source.clone();
        assert!(!apply(&mut source, &CodemodContext::current()).unwrap());
        assert_eq!(source, before);
    }
}
