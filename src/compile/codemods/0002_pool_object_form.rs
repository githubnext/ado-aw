//! `pool: <string>` → explicit object form
//!
//! Non-1ES targets now support both self-hosted (`name`) and
//! Microsoft-hosted (`vmImage`) pool syntax. This codemod rewrites the
//! legacy scalar shorthand into an explicit object form so sources are
//! unambiguous and easier to evolve.
//!
//! When the pool field is **absent** and the compiler version is at or
//! above `INTRODUCED_IN` (the release that changed the implicit
//! default from the 1ES self-hosted pool to `vmImage: ubuntu-22.04`),
//! the codemod pins the legacy default explicitly so existing
//! pipelines are not silently broken.

use anyhow::Result;
use serde_yaml::{Mapping, Value};

use super::{Codemod, CodemodContext};
use crate::compile::common::DEFAULT_ONEES_POOL;

/// Version where the pool default changed from the legacy self-hosted
/// pool to `vmImage: ubuntu-22.04`.
const INTRODUCED_IN: &str = "0.30.0";

pub static CODEMOD: Codemod = Codemod {
    id: "pool_object_form",
    summary: "pool: <string> -> pool object form (name/vmImage)",
    introduced_in: INTRODUCED_IN,
    apply: apply_codemod,
};

/// Simple major.minor.patch comparison. Returns true when `version`
/// is greater than or equal to `threshold`.
fn version_gte(version: &str, threshold: &str) -> bool {
    let parse = |s: &str| -> (u32, u32, u32) {
        let mut parts = s.split('.');
        let major = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
        let minor = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
        let patch = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
        (major, minor, patch)
    };
    parse(version) >= parse(threshold)
}

fn apply_codemod(fm: &mut Mapping, ctx: &CodemodContext) -> Result<bool> {
    let key = Value::String("pool".to_string());

    let Some(pool_value) = fm.get(&key).cloned() else {
        // Pool absent — only inject the legacy default for 1ES
        // targets where the old implicit default was the self-hosted
        // pool. Non-1ES (standalone/job/stage) targets now default to
        // `vmImage: ubuntu-22.04`, which is the desired behaviour
        // for new pipelines that omit `pool:`.
        if !version_gte(ctx.compiler_version, INTRODUCED_IN) {
            return Ok(false);
        }
        let target = fm
            .get(&Value::String("target".to_string()))
            .and_then(|v| v.as_str());
        if target != Some("1es") {
            return Ok(false);
        }
        let mut mapped = Mapping::new();
        mapped.insert(
            Value::String("name".to_string()),
            Value::String(DEFAULT_ONEES_POOL.to_string()),
        );
        fm.insert(key, Value::Mapping(mapped));
        return Ok(true);
    };

    let Value::String(name) = pool_value else {
        // Already object-form (or invalid in another way) — no-op.
        return Ok(false);
    };

    let mut mapped = Mapping::new();
    mapped.insert(Value::String("name".to_string()), Value::String(name));
    fm.insert(key, Value::Mapping(mapped));
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a context with an explicit version for testing.
    fn ctx(version: &'static str) -> CodemodContext {
        CodemodContext {
            compiler_version: version,
        }
    }

    #[test]
    fn noops_when_pool_is_already_mapping() {
        let mut fm: Mapping =
            serde_yaml::from_str("name: x\ndescription: y\npool:\n  vmImage: ubuntu-22.04")
                .unwrap();
        let changed = apply_codemod(&mut fm, &ctx("0.30.0")).expect("apply");
        assert!(!changed);
        assert_eq!(
            fm.get(Value::String("pool".into())).cloned(),
            Some(serde_yaml::from_str::<Value>("vmImage: ubuntu-22.04").unwrap())
        );
    }

    #[test]
    fn inserts_legacy_default_when_pool_absent_1es_and_version_gte() {
        let mut fm: Mapping =
            serde_yaml::from_str("name: x\ndescription: y\ntarget: 1es").unwrap();
        let changed = apply_codemod(&mut fm, &ctx("0.30.0")).expect("apply");
        assert!(changed);
        assert_eq!(
            fm.get(Value::String("pool".into())).cloned(),
            Some(serde_yaml::from_str::<Value>("name: AZS-1ES-L-MMS-ubuntu-22.04").unwrap())
        );
    }

    #[test]
    fn noops_when_pool_absent_standalone_and_version_gte() {
        // Neither implicit nor explicit `standalone` target should
        // ever receive the legacy 1ES pool injection.
        for yaml in &[
            "name: x\ndescription: y",
            "name: x\ndescription: y\ntarget: standalone",
        ] {
            let mut fm: Mapping = serde_yaml::from_str(yaml).unwrap();
            let changed = apply_codemod(&mut fm, &ctx("0.30.0")).expect("apply");
            assert!(!changed, "yaml: {}", yaml);
            assert!(!fm.contains_key(Value::String("pool".into())), "yaml: {}", yaml);
        }
    }

    #[test]
    fn noops_when_pool_absent_1es_and_version_below() {
        let mut fm: Mapping =
            serde_yaml::from_str("name: x\ndescription: y\ntarget: 1es").unwrap();
        let changed = apply_codemod(&mut fm, &ctx("0.29.0")).expect("apply");
        assert!(!changed);
        assert!(!fm.contains_key(Value::String("pool".into())));
    }

    #[test]
    fn idempotent_after_inserting_legacy_default() {
        let mut fm: Mapping =
            serde_yaml::from_str("name: x\ndescription: y\ntarget: 1es").unwrap();
        let changed1 = apply_codemod(&mut fm, &ctx("0.30.0")).expect("first apply");
        assert!(changed1);
        let changed2 = apply_codemod(&mut fm, &ctx("0.30.0")).expect("second apply");
        assert!(!changed2, "second run must be a no-op");
    }

    #[test]
    fn rewrites_legacy_default_pool_to_name_object() {
        let mut fm: Mapping =
            serde_yaml::from_str("name: x\ndescription: y\npool: AZS-1ES-L-MMS-ubuntu-22.04")
                .unwrap();
        let changed = apply_codemod(&mut fm, &ctx("0.30.0")).expect("apply");
        assert!(changed);
        assert_eq!(
            fm.get(Value::String("pool".into())).cloned(),
            Some(serde_yaml::from_str::<Value>("name: AZS-1ES-L-MMS-ubuntu-22.04").unwrap())
        );
    }

    #[test]
    fn scalar_rewrite_applies_regardless_of_version() {
        // Scalar → object rewrite is unconditional; only the
        // absent-pool injection is version-gated.
        let mut fm: Mapping =
            serde_yaml::from_str("name: x\ndescription: y\npool: MyPool").unwrap();
        let changed = apply_codemod(&mut fm, &ctx("0.28.0")).expect("apply");
        assert!(changed);
        assert_eq!(
            fm.get(Value::String("pool".into())).cloned(),
            Some(serde_yaml::from_str::<Value>("name: MyPool").unwrap())
        );
    }

    #[test]
    fn version_gte_comparisons() {
        assert!(version_gte("0.30.0", "0.30.0"));
        assert!(version_gte("0.31.0", "0.30.0"));
        assert!(version_gte("1.0.0", "0.30.0"));
        assert!(version_gte("0.30.1", "0.30.0"));
        assert!(!version_gte("0.29.0", "0.30.0"));
        assert!(!version_gte("0.29.99", "0.30.0"));
    }
}
