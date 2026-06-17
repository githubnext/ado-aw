//! `safe-outputs.noop.work-item:` / `safe-outputs.missing-tool.work-item:` → flat form
//!
//! Before this codemod, diagnostic safe outputs configured work-item
//! filing via a nested `work-item:` sub-block:
//!
//! ```yaml
//! safe-outputs:
//!   noop:
//!     work-item:
//!       title: "[ado-aw] Agent reported no operation"
//!       work-item-type: Task
//!       area-path: "MyProject\\MyTeam"
//!       tags: [agent-noop]
//! ```
//!
//! The new flat form (aligned with gh-aw's convention) hoists these
//! fields to the parent level and renames `title` → `title-prefix`:
//!
//! ```yaml
//! safe-outputs:
//!   noop:
//!     title-prefix: "[ado-aw] Agent reported no operation"
//!     work-item-type: Task
//!     area-path: "MyProject\\MyTeam"
//!     tags: [agent-noop]
//! ```
//!
//! The `enabled: false` field is migrated to `report-as-work-item: false`.

use anyhow::{Result, bail};
use serde_yaml::{Mapping, Value};

use super::{Codemod, CodemodContext};

const INTRODUCED_IN: &str = "0.37.0";

pub static CODEMOD: Codemod = Codemod {
    id: "flatten_work_item_config",
    summary: "safe-outputs.{noop,missing-tool}.work-item -> flat fields (title-prefix, area-path, etc.)",
    introduced_in: INTRODUCED_IN,
    apply: apply_codemod,
};

/// The diagnostic tool keys that may carry a nested `work-item:` block.
const DIAGNOSTIC_KEYS: &[&str] = &["noop", "missing-tool", "missing-data"];

fn apply_codemod(fm: &mut Mapping, _ctx: &CodemodContext) -> Result<bool> {
    let so_key = Value::String("safe-outputs".to_string());
    let Some(so_val) = fm.get_mut(&so_key) else {
        return Ok(false);
    };
    let Some(so_map) = so_val.as_mapping_mut() else {
        return Ok(false);
    };

    let mut changed = false;

    for &tool_key_str in DIAGNOSTIC_KEYS {
        let tool_key = Value::String(tool_key_str.to_string());
        let Some(tool_val) = so_map.get_mut(&tool_key) else {
            continue;
        };
        let Some(tool_map) = tool_val.as_mapping_mut() else {
            continue;
        };

        let wi_key = Value::String("work-item".to_string());
        let Some(wi_val) = tool_map.remove(&wi_key) else {
            continue;
        };
        let Value::Mapping(wi_map) = wi_val else {
            // work-item was present but not a mapping — put it back as-is
            continue;
        };

        // Check for conflict: if flat fields already exist alongside work-item:
        let flat_keys = ["title-prefix", "work-item-type", "area-path", "iteration-path", "tags", "report-as-work-item"];
        for &fk in &flat_keys {
            if tool_map.contains_key(Value::String(fk.to_string())) {
                bail!(
                    "safe-outputs.{}.work-item and safe-outputs.{}.{} both present — \
                     manual migration required (remove the nested work-item: block)",
                    tool_key_str, tool_key_str, fk
                );
            }
        }

        // Hoist fields from work-item: to the parent level
        for (k, v) in wi_map {
            let key_str = k.as_str().unwrap_or_default().to_string();
            let new_key = match key_str.as_str() {
                "title" => "title-prefix".to_string(),
                "enabled" => "report-as-work-item".to_string(),
                other => other.to_string(),
            };
            tool_map.insert(Value::String(new_key), v);
        }

        changed = true;
    }

    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::codemods::CodemodContext;

    fn ctx() -> CodemodContext {
        CodemodContext {
            compiler_version: "0.37.0",
        }
    }

    #[test]
    fn flattens_noop_work_item_block() {
        let yaml = r#"
name: Test
description: Test
safe-outputs:
  noop:
    work-item:
      title: "[ado-aw] Agent reported no operation"
      work-item-type: Task
      area-path: "MyProject\\MyTeam"
      tags:
        - agent-noop
"#;
        let mut fm: Mapping = serde_yaml::from_str(yaml).unwrap();
        let changed = apply_codemod(&mut fm, &ctx()).unwrap();
        assert!(changed);

        let so = fm.get(Value::String("safe-outputs".into())).unwrap().as_mapping().unwrap();
        let noop = so.get(Value::String("noop".into())).unwrap().as_mapping().unwrap();

        // work-item: key should be gone
        assert!(!noop.contains_key(Value::String("work-item".into())));
        // Flat fields should be present
        assert_eq!(
            noop.get(Value::String("title-prefix".into())),
            Some(&Value::String("[ado-aw] Agent reported no operation".into()))
        );
        assert_eq!(
            noop.get(Value::String("work-item-type".into())),
            Some(&Value::String("Task".into()))
        );
        assert_eq!(
            noop.get(Value::String("area-path".into())),
            Some(&Value::String("MyProject\\MyTeam".into()))
        );
    }

    #[test]
    fn flattens_missing_tool_work_item_block() {
        let yaml = r#"
name: Test
description: Test
safe-outputs:
  missing-tool:
    work-item:
      title: "[ado-aw] Missing tool"
      enabled: false
"#;
        let mut fm: Mapping = serde_yaml::from_str(yaml).unwrap();
        let changed = apply_codemod(&mut fm, &ctx()).unwrap();
        assert!(changed);

        let so = fm.get(Value::String("safe-outputs".into())).unwrap().as_mapping().unwrap();
        let mt = so.get(Value::String("missing-tool".into())).unwrap().as_mapping().unwrap();

        assert!(!mt.contains_key(Value::String("work-item".into())));
        assert_eq!(
            mt.get(Value::String("title-prefix".into())),
            Some(&Value::String("[ado-aw] Missing tool".into()))
        );
        assert_eq!(
            mt.get(Value::String("report-as-work-item".into())),
            Some(&Value::Bool(false))
        );
    }

    #[test]
    fn noops_when_no_work_item_block() {
        let yaml = r#"
name: Test
description: Test
safe-outputs:
  noop: {}
  missing-tool: {}
"#;
        let mut fm: Mapping = serde_yaml::from_str(yaml).unwrap();
        let changed = apply_codemod(&mut fm, &ctx()).unwrap();
        assert!(!changed);
    }

    #[test]
    fn noops_when_no_safe_outputs() {
        let yaml = "name: Test\ndescription: Test\n";
        let mut fm: Mapping = serde_yaml::from_str(yaml).unwrap();
        let changed = apply_codemod(&mut fm, &ctx()).unwrap();
        assert!(!changed);
    }

    #[test]
    fn errors_on_conflict() {
        let yaml = r#"
name: Test
description: Test
safe-outputs:
  noop:
    title-prefix: "existing"
    work-item:
      title: "conflicting"
"#;
        let mut fm: Mapping = serde_yaml::from_str(yaml).unwrap();
        let err = apply_codemod(&mut fm, &ctx()).unwrap_err();
        assert!(
            format!("{}", err).contains("manual migration required"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn idempotent_after_flatten() {
        let yaml = r#"
name: Test
description: Test
safe-outputs:
  noop:
    work-item:
      title: "[ado-aw] Noop"
      work-item-type: Bug
"#;
        let mut fm: Mapping = serde_yaml::from_str(yaml).unwrap();
        let changed1 = apply_codemod(&mut fm, &ctx()).unwrap();
        assert!(changed1);
        let snapshot = fm.clone();
        let changed2 = apply_codemod(&mut fm, &ctx()).unwrap();
        assert!(!changed2);
        assert_eq!(fm, snapshot);
    }
}
