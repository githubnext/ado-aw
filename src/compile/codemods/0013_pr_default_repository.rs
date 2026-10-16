use anyhow::Result;
use serde_yaml::{Mapping, Value};

use super::{Codemod, CodemodContext, ConflictPolicy, rename_key};

pub static CODEMOD: Codemod = Codemod {
    id: "pr_default_repository",
    summary: "rename ADO PR target-repo to default-repository; allowed-repositories remains the authority",
    introduced_in: super::m0011_explicit_pr_policy::INTRODUCED_IN,
    apply,
};

fn apply(front_matter: &mut Mapping, _ctx: &CodemodContext) -> Result<bool> {
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
    let mut migrated = front_matter.clone();
    let Some(Value::Mapping(outputs)) = migrated.get_mut("safe-outputs") else {
        return Ok(false);
    };
    for tool in crate::safe_outputs::pr_common::PR_MUTATION_TOOLS {
        if custom.contains(*tool) {
            continue;
        }
        let Some(Value::Mapping(config)) = outputs.get_mut(*tool) else {
            continue;
        };
        rename_key(
            config,
            "target-repo",
            "default-repository",
            ConflictPolicy::Error,
        )?;
        if *tool == "abandon-pull-request" {
            rename_key(
                config,
                "repository",
                "default-repository",
                ConflictPolicy::Error,
            )?;
        }
    }
    let changed = *front_matter != migrated;
    if changed {
        *front_matter = migrated;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_renamed_without_touching_grants_or_other_tool_families() {
        let mut source: Mapping = serde_yaml::from_str(
            "safe-outputs:\n  add-pull-request-comment: {target-repo: self, allowed-repositories: [other]}\n  create-github-issue: {target-repo: owner/repo}\n"
        ).unwrap();
        assert!(apply(&mut source, &CodemodContext::current()).unwrap());
        let config = &source["safe-outputs"]["add-pull-request-comment"];
        assert_eq!(config["default-repository"], "self");
        assert_eq!(config["allowed-repositories"][0], "other");
        assert_eq!(
            source["safe-outputs"]["create-github-issue"]["target-repo"],
            "owner/repo"
        );
        assert!(!apply(&mut source, &CodemodContext::current()).unwrap());
    }

    #[test]
    fn conflicting_defaults_fail_atomically() {
        let mut source: Mapping = serde_yaml::from_str(
            "safe-outputs:\n  add-pull-request-comment: {target-repo: self, default-repository: other}\n"
        ).unwrap();
        let original = source.clone();
        assert!(apply(&mut source, &CodemodContext::current()).is_err());
        assert_eq!(source, original);
    }
}
