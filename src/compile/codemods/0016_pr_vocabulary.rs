use super::{Codemod, CodemodContext, ConflictPolicy, rename_key};
use anyhow::{Result, ensure};
use serde_yaml::{Mapping, Value};

pub static CODEMOD: Codemod = Codemod {
    id: "pr_vocabulary",
    summary: "remove inert PR branch-update configuration and rename footer to include-stats; PR clients must use body and pull_request_id",
    introduced_in: super::m0011_explicit_pr_policy::INTRODUCED_IN,
    apply,
};

fn apply(front_matter: &mut Mapping, _ctx: &CodemodContext) -> Result<bool> {
    let Some(config) = front_matter
        .get("safe-outputs")
        .and_then(|outputs| outputs.get("update-pull-request"))
    else {
        return Ok(false);
    };
    if !config.is_mapping() {
        return Ok(false);
    }
    if crate::compile::imports::pr_policy::local_custom_job_names(front_matter)?
        .contains("update-pull-request")
    {
        return Ok(false);
    }
    let mut updated = config.as_mapping().expect("mapping checked").clone();
    if let Some(value) = updated.remove("update-branch") {
        ensure!(
            value.as_bool() == Some(false),
            "update-pull-request.update-branch is not supported; only the old inert false setting can be removed"
        );
    }
    rename_key(
        &mut updated,
        "footer",
        "include-stats",
        ConflictPolicy::Error,
    )?;
    let changed = config != &Value::Mapping(updated.clone());
    if changed {
        front_matter
            .get_mut("safe-outputs")
            .and_then(Value::as_mapping_mut)
            .expect("safe-outputs mapping checked")
            .insert("update-pull-request".into(), Value::Mapping(updated));
    }
    Ok(changed)
}
