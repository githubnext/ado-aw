use anyhow::Result;
use serde_yaml::Mapping;

use super::{Codemod, CodemodContext};

pub static CODEMOD: Codemod = Codemod {
    id: "split_update_pr",
    summary: "split update-pr into focused PR tools while preserving policy and shared budgets",
    introduced_in: env!("CARGO_PKG_VERSION"),
    apply,
};

fn apply(front_matter: &mut Mapping, _ctx: &CodemodContext) -> Result<bool> {
    if front_matter
        .get("safe-outputs")
        .and_then(|outputs| outputs.get("update-pr"))
        .is_none()
    {
        return Ok(false);
    }
    let custom_jobs = crate::compile::imports::pr_policy::local_custom_job_names(front_matter)?;
    crate::compile::imports::pr_policy::transform_builtins(
        front_matter,
        &custom_jobs,
        crate::compile::pr_migration::migrate_safe_outputs,
    )
}
