use anyhow::Result;
use serde_yaml::Mapping;

use super::{Codemod, CodemodContext};

pub static CODEMOD: Codemod = Codemod {
    id: "pull_request_tool_names",
    summary: "expand abbreviated PR tool names to pull-request, including shared budget references",
    introduced_in: env!("CARGO_PKG_VERSION"),
    apply,
};

fn apply(front_matter: &mut Mapping, _ctx: &CodemodContext) -> Result<bool> {
    let Some(outputs) = front_matter.get("safe-outputs") else {
        return Ok(false);
    };
    if !crate::compile::pr_migration::PR_TOOL_RENAMES
        .iter()
        .any(|(old, _)| outputs.get(*old).is_some())
        && outputs.get("budget-groups").is_none()
    {
        return Ok(false);
    }
    let custom_jobs = crate::compile::imports::pr_policy::local_custom_job_names(front_matter)?;
    crate::compile::imports::pr_policy::rename_declarations(front_matter, &custom_jobs)
}
