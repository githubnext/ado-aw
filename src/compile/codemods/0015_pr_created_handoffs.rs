use super::{Codemod, CodemodContext};
use anyhow::{Result, ensure};
use serde_yaml::{Mapping, Value};

pub static CODEMOD: Codemod = Codemod {
    id: "pr_created_handoffs",
    summary: "remove per-tool temporary-ID switches; PR handles remain constrained by target, repository and actual dependency lanes",
    introduced_in: super::m0011_explicit_pr_policy::INTRODUCED_IN,
    apply,
};

fn apply(front_matter: &mut Mapping, _ctx: &CodemodContext) -> Result<bool> {
    let tools = [
        "add-pull-request-comment",
        "reply-to-pull-request-comment",
        "resolve-pull-request-thread",
        "submit-pull-request-review",
    ];
    if !tools.iter().any(|tool| {
        front_matter
            .get("safe-outputs")
            .and_then(|outputs| outputs.get(*tool))
            .is_some()
    }) {
        return Ok(false);
    }
    let custom = crate::compile::imports::pr_policy::local_custom_job_names(front_matter)?;
    let mut migrated = front_matter.clone();
    let Some(Value::Mapping(outputs)) = migrated.get_mut("safe-outputs") else {
        return Ok(false);
    };
    for tool in tools {
        if custom.contains(tool) {
            continue;
        }
        let Some(Value::Mapping(config)) = outputs.get_mut(tool) else {
            continue;
        };
        if let Some(value) = config.remove("allow-temporary-ids") {
            ensure!(
                value.as_bool().is_some(),
                "allow-temporary-ids must be a boolean before migration"
            );
        }
    }
    let changed = migrated != *front_matter;
    if changed {
        *front_matter = migrated;
    }
    Ok(changed)
}
