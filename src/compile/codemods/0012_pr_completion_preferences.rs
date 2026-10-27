use anyhow::{Context, Result, ensure};
use serde_yaml::{Mapping, Value};

use super::{Codemod, CodemodContext};

pub static CODEMOD: Codemod = Codemod {
    id: "pr_completion_preferences",
    summary: "preserve explicit PR completion preferences; disable ineffective legacy draft auto-complete without publishing the draft",
    introduced_in: super::m0011_explicit_pr_policy::INTRODUCED_IN,
    apply,
};

fn apply(front_matter: &mut Mapping, ctx: &CodemodContext) -> Result<bool> {
    if !["create-pull-request", "set-pull-request-auto-complete"]
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
    let old_source = ctx
        .source_compiler_version
        .as_deref()
        .is_some_and(|version| crate::version::is_older_than(version, CODEMOD.introduced_in));
    let mut migrated = front_matter.clone();
    let Some(Value::Mapping(outputs)) = migrated.get_mut("safe-outputs") else {
        return Ok(false);
    };
    for tool in ["create-pull-request", "set-pull-request-auto-complete"] {
        if custom.contains(tool) {
            continue;
        }
        let Some(raw) = outputs.get_mut(tool) else {
            continue;
        };
        if old_source && tool == "create-pull-request" && raw.is_null() {
            *raw = Value::Mapping(Mapping::new());
        }
        let Some(config) = raw.as_mapping_mut() else {
            continue;
        };
        let old_shape = tool == "create-pull-request" && config.contains_key("squash-merge");
        if old_shape {
            super::validate_legacy_literal_names(tool, config)?;
            ensure!(
                !config.contains_key("merge-strategy"),
                "manual migration required: both squash-merge and merge-strategy are configured"
            );
            let squash = config
                .remove("squash-merge")
                .and_then(|value| value.as_bool())
                .context("create-pull-request.squash-merge must be a boolean")?;
            config.insert(
                "merge-strategy".into(),
                if squash { "squash" } else { "noFastForward" }.into(),
            );
        }
        if (old_source && tool == "create-pull-request") || old_shape {
            if tool == "create-pull-request" {
                super::preserve_legacy_grants(tool, config);
            }
            config
                .entry(Value::String("merge-strategy".into()))
                .or_insert(Value::String("squash".into()));
            config
                .entry(Value::String("delete-source-branch".into()))
                .or_insert(Value::Bool(true));
            if tool == "create-pull-request" {
                let draft = config
                    .get("draft")
                    .map(|value| {
                        value
                            .as_bool()
                            .context("create-pull-request.draft must be a boolean")
                    })
                    .transpose()?
                    .unwrap_or(true);
                let complete = config
                    .get("auto-complete")
                    .map(|value| {
                        value
                            .as_bool()
                            .context("create-pull-request.auto-complete must be a boolean")
                    })
                    .transpose()?
                    .unwrap_or(false);
                if draft && complete {
                    config.insert("auto-complete".into(), Value::Bool(false));
                }
            }
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
    fn legacy_defaults_are_explicit_and_inactive_completion_stays_off() {
        let mut source: Mapping = serde_yaml::from_str(
            "safe-outputs:\n  create-pull-request:\n    auto-complete: true\n    delete-source-branch: false\n").unwrap();
        let ctx = CodemodContext::for_source(Some("0.53.0".into()));
        assert!(apply(&mut source, &ctx).unwrap());
        assert_eq!(
            source["safe-outputs"]["create-pull-request"]["auto-complete"],
            false
        );
        assert_eq!(
            source["safe-outputs"]["create-pull-request"]["delete-source-branch"],
            false
        );
        assert_eq!(
            source["safe-outputs"]["create-pull-request"]["merge-strategy"],
            "squash"
        );
        assert!(!apply(&mut source, &ctx).unwrap());
    }

    #[test]
    fn new_or_unknown_provenance_does_not_repair_draft_conflicts() {
        for version in [None, Some("unknown"), Some(CODEMOD.introduced_in)] {
            let mut source: Mapping = serde_yaml::from_str(
                "safe-outputs:\n  create-pull-request: {draft: true, auto-complete: true}\n",
            )
            .unwrap();
            let original = source.clone();
            assert!(
                !apply(
                    &mut source,
                    &CodemodContext::for_source(version.map(str::to_owned))
                )
                .unwrap()
            );
            assert_eq!(source, original);
        }
    }

    #[test]
    fn obsolete_squash_boolean_migrates_and_conflicts_are_atomic() {
        for (squash, strategy) in [(true, "squash"), (false, "noFastForward")] {
            let mut source: Mapping = serde_yaml::from_str(&format!(
                "safe-outputs:\n  create-pull-request: {{draft: false, auto-complete: true, squash-merge: {squash}}}\n"
            )).unwrap();
            assert!(apply(&mut source, &CodemodContext::current()).unwrap());
            assert_eq!(
                source["safe-outputs"]["create-pull-request"]["merge-strategy"],
                strategy
            );
            assert_eq!(
                source["safe-outputs"]["create-pull-request"]["auto-complete"],
                true
            );
            assert!(!apply(&mut source, &CodemodContext::current()).unwrap());
        }
        let mut conflict: Mapping = serde_yaml::from_str(
            "safe-outputs:\n  create-pull-request: {squash-merge: false, merge-strategy: squash}\n",
        )
        .unwrap();
        let original = conflict.clone();
        assert!(apply(&mut conflict, &CodemodContext::current()).is_err());
        assert_eq!(conflict, original);
    }
}
