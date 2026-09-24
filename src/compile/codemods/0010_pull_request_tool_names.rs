use anyhow::Result;
use serde_yaml::{Mapping, Value};

use super::{Codemod, CodemodContext};

pub static CODEMOD: Codemod = Codemod {
    id: "pull_request_tool_names",
    summary: "expand abbreviated PR tool names to pull-request, including shared budget references",
    introduced_in: env!("CARGO_PKG_VERSION"),
    apply,
};

fn apply(front_matter: &mut Mapping, _ctx: &CodemodContext) -> Result<bool> {
    let key = Value::String("safe-outputs".to_string());
    let Some(raw) = front_matter.get(&key) else {
        return Ok(false);
    };
    let value = serde_json::to_value(raw)?;
    let Some(mut outputs) = value.as_object().cloned() else {
        return Ok(false);
    };
    if !crate::compile::pr_migration::rename_pr_tools(&mut outputs)? {
        return Ok(false);
    }
    front_matter.insert(key, serde_yaml::to_value(outputs)?);
    Ok(true)
}
