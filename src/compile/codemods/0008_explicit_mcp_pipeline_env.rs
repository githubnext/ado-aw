//! `mcp-servers.*.env.NAME: ""` -> explicit pipeline-variable source.

use anyhow::Result;
use serde_yaml::{Mapping, Value};

use super::{Codemod, CodemodContext};

const INTRODUCED_IN: &str = "0.51.0";

pub static CODEMOD: Codemod = Codemod {
    id: "explicit_mcp_pipeline_env",
    summary: "empty MCP env passthrough values moved to explicit pipeline-variable objects",
    introduced_in: INTRODUCED_IN,
    apply: apply_codemod,
};

fn key(name: &str) -> Value {
    Value::String(name.to_string())
}

fn pipeline_variable(name: &str) -> Value {
    Value::Mapping(Mapping::from_iter([(
        key("pipeline-variable"),
        Value::String(name.to_string()),
    )]))
}

fn apply_codemod(fm: &mut Mapping, _ctx: &CodemodContext) -> Result<bool> {
    let Some(servers) = fm
        .get_mut(key("mcp-servers"))
        .and_then(Value::as_mapping_mut)
    else {
        return Ok(false);
    };

    let mut changed = false;
    for server in servers.values_mut() {
        let Some(server) = server.as_mapping_mut() else {
            continue;
        };
        let Some(env) = server.get_mut(key("env")).and_then(Value::as_mapping_mut) else {
            continue;
        };
        for (name, value) in env.iter_mut() {
            if value.as_str() != Some("") {
                continue;
            }
            let Some(name) = name.as_str() else {
                continue;
            };
            *value = pipeline_variable(name);
            changed = true;
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> CodemodContext {
        CodemodContext {
            compiler_version: INTRODUCED_IN,
            source_compiler_version: None,
        }
    }

    fn map(yaml: &str) -> Mapping {
        serde_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn rewrites_every_empty_env_value_and_preserves_literals() {
        let mut fm = map(
            "mcp-servers:\n  one:\n    container: image\n    env:\n      TOKEN: \"\"\n      STATIC: value\n  two:\n    container: image\n    env:\n      OTHER: \"\"\n",
        );
        assert!(apply_codemod(&mut fm, &ctx()).unwrap());
        assert_eq!(
            fm["mcp-servers"]["one"]["env"]["TOKEN"]["pipeline-variable"],
            "TOKEN"
        );
        assert_eq!(fm["mcp-servers"]["one"]["env"]["STATIC"], "value");
        assert_eq!(
            fm["mcp-servers"]["two"]["env"]["OTHER"]["pipeline-variable"],
            "OTHER"
        );
    }

    #[test]
    fn explicit_and_non_mapping_shapes_are_noops() {
        let mut fm = map(
            "mcp-servers:\n  explicit:\n    container: image\n    env:\n      TOKEN:\n        pipeline-variable: SOURCE\n  scalar: true\n",
        );
        let snapshot = fm.clone();
        assert!(!apply_codemod(&mut fm, &ctx()).unwrap());
        assert_eq!(fm, snapshot);
    }

    #[test]
    fn migration_is_idempotent() {
        let mut fm =
            map("mcp-servers:\n  one:\n    container: image\n    env:\n      TOKEN: \"\"\n");
        assert!(apply_codemod(&mut fm, &ctx()).unwrap());
        let snapshot = fm.clone();
        assert!(!apply_codemod(&mut fm, &ctx()).unwrap());
        assert_eq!(fm, snapshot);
    }
}
