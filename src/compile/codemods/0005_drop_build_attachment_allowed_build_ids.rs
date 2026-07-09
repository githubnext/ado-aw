//! `safe-outputs.upload-build-attachment.allowed-build-ids:` -> removed
//!
//! A build attachment is a DistributedTask **timeline attachment** on the
//! current job's record (the same object `##vso[task.addattachment]` creates),
//! so it can only ever be added to the **current** run. The historical
//! `allowed-build-ids` allow-list — which gated *which other* build the agent
//! could attach to — is therefore meaningless: no other build is reachable.
//!
//! This codemod strips the now-inert key from
//! `safe-outputs.upload-build-attachment` and lets the compile warning tell the
//! author it was auto-removed. It deliberately does **not** touch
//! `safe-outputs.upload-pipeline-artifact.allowed-build-ids`, which remains
//! meaningful (pipeline artifacts use the Build Artifacts `Create` API, which
//! can target an arbitrary build).
//!
//! Before:
//!
//! ```yaml
//! safe-outputs:
//!   upload-build-attachment:
//!     allowed-build-ids:
//!       - 12345
//!     max-file-size: 1048576
//! ```
//!
//! After:
//!
//! ```yaml
//! safe-outputs:
//!   upload-build-attachment:
//!     max-file-size: 1048576
//! ```

use anyhow::Result;
use serde_yaml::{Mapping, Value};

use super::{Codemod, CodemodContext};

const INTRODUCED_IN: &str = "0.42.0";

pub static CODEMOD: Codemod = Codemod {
    id: "drop_build_attachment_allowed_build_ids",
    summary: "safe-outputs.upload-build-attachment.allowed-build-ids removed \
              (build attachments are current-run only)",
    introduced_in: INTRODUCED_IN,
    apply: apply_codemod,
};

fn apply_codemod(fm: &mut Mapping, _ctx: &CodemodContext) -> Result<bool> {
    let so_key = Value::String("safe-outputs".to_string());
    let Some(so_val) = fm.get_mut(&so_key) else {
        return Ok(false);
    };
    let Some(so_map) = so_val.as_mapping_mut() else {
        return Ok(false);
    };

    let tool_key = Value::String("upload-build-attachment".to_string());
    let Some(tool_val) = so_map.get_mut(&tool_key) else {
        return Ok(false);
    };
    let Some(tool_map) = tool_val.as_mapping_mut() else {
        return Ok(false);
    };

    let removed = tool_map
        .remove(Value::String("allowed-build-ids".to_string()))
        .is_some();
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::codemods::CodemodContext;

    fn ctx() -> CodemodContext {
        CodemodContext {
            compiler_version: INTRODUCED_IN,
        }
    }

    fn map(yaml: &str) -> Mapping {
        serde_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn removes_allowed_build_ids_from_upload_build_attachment() {
        let mut m = map(
            "safe-outputs:\n  upload-build-attachment:\n    allowed-build-ids:\n      - 12345\n      - 67890\n    max-file-size: 1048576\n",
        );
        assert!(apply_codemod(&mut m, &ctx()).unwrap());
        let so = m
            .get(Value::String("safe-outputs".into()))
            .unwrap()
            .as_mapping()
            .unwrap();
        let tool = so
            .get(Value::String("upload-build-attachment".into()))
            .unwrap()
            .as_mapping()
            .unwrap();
        assert!(!tool.contains_key(Value::String("allowed-build-ids".into())));
        // Sibling config is preserved.
        assert!(tool.contains_key(Value::String("max-file-size".into())));
    }

    #[test]
    fn leaves_upload_pipeline_artifact_untouched() {
        let mut m = map(
            "safe-outputs:\n  upload-pipeline-artifact:\n    allowed-build-ids:\n      - 12345\n",
        );
        assert!(!apply_codemod(&mut m, &ctx()).unwrap());
        let so = m
            .get(Value::String("safe-outputs".into()))
            .unwrap()
            .as_mapping()
            .unwrap();
        let tool = so
            .get(Value::String("upload-pipeline-artifact".into()))
            .unwrap()
            .as_mapping()
            .unwrap();
        assert!(
            tool.contains_key(Value::String("allowed-build-ids".into())),
            "upload-pipeline-artifact.allowed-build-ids must be preserved"
        );
    }

    #[test]
    fn noop_when_key_absent() {
        let mut m = map("safe-outputs:\n  upload-build-attachment:\n    max-file-size: 1024\n");
        assert!(!apply_codemod(&mut m, &ctx()).unwrap());
    }

    #[test]
    fn noop_when_no_safe_outputs() {
        let mut m = map("name: x\ndescription: y\n");
        assert!(!apply_codemod(&mut m, &ctx()).unwrap());
    }

    #[test]
    fn idempotent() {
        let mut m = map(
            "safe-outputs:\n  upload-build-attachment:\n    allowed-build-ids:\n      - 1\n",
        );
        assert!(apply_codemod(&mut m, &ctx()).unwrap());
        let snapshot = m.clone();
        assert!(!apply_codemod(&mut m, &ctx()).unwrap());
        assert_eq!(m, snapshot, "second run must not mutate the mapping");
    }
}
