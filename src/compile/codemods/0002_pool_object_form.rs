//! `pool: <string>` → explicit object form
//!
//! Non-1ES targets now support both self-hosted (`name`) and
//! Microsoft-hosted (`vmImage`) pool syntax. This codemod rewrites the
//! legacy scalar shorthand into an explicit object form so sources are
//! unambiguous and easier to evolve.

use anyhow::Result;
use serde_yaml::{Mapping, Value};

use super::{Codemod, CodemodContext};

pub static CODEMOD: Codemod = Codemod {
    id: "pool_object_form",
    summary: "pool: <string> -> pool object form (name/vmImage)",
    introduced_in: "0.30.0",
    apply: apply_codemod,
};

const LEGACY_DEFAULT_POOL_NAME: &str = "AZS-1ES-L-MMS-ubuntu-22.04";
const NON_ONEES_DEFAULT_VM_IMAGE: &str = "ubuntu-latest";

fn is_1es_target(fm: &Mapping) -> bool {
    fm.get(Value::String("target".to_string()))
        .and_then(Value::as_str)
        .is_some_and(|v| v.eq_ignore_ascii_case("1es"))
}

fn apply_codemod(fm: &mut Mapping, _ctx: &CodemodContext) -> Result<bool> {
    let key = Value::String("pool".to_string());
    let Some(pool_value) = fm.get(&key).cloned() else {
        return Ok(false);
    };

    let Value::String(name) = pool_value else {
        // Already object-form (or invalid in another way) — no-op.
        return Ok(false);
    };

    let mut mapped = Mapping::new();
    // Translate the historical default for non-1ES sources to the new
    // non-1ES default shape.
    if name == LEGACY_DEFAULT_POOL_NAME && !is_1es_target(fm) {
        mapped.insert(
            Value::String("vmImage".to_string()),
            Value::String(NON_ONEES_DEFAULT_VM_IMAGE.to_string()),
        );
    } else {
        mapped.insert(Value::String("name".to_string()), Value::String(name));
    }
    fm.insert(key, Value::Mapping(mapped));
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_scalar_pool_to_name_object() {
        let mut fm: Mapping = serde_yaml::from_str("name: x\ndescription: y\npool: MyPool").unwrap();
        let changed = apply_codemod(&mut fm, &CodemodContext {}).expect("apply");
        assert!(changed);
        assert_eq!(
            fm.get(Value::String("pool".into())).cloned(),
            Some(serde_yaml::from_str::<Value>("name: MyPool").unwrap())
        );
    }

    #[test]
    fn noops_when_pool_is_already_mapping() {
        let mut fm: Mapping =
            serde_yaml::from_str("name: x\ndescription: y\npool:\n  vmImage: ubuntu-latest")
                .unwrap();
        let changed = apply_codemod(&mut fm, &CodemodContext {}).expect("apply");
        assert!(!changed);
        assert_eq!(
            fm.get(Value::String("pool".into())).cloned(),
            Some(serde_yaml::from_str::<Value>("vmImage: ubuntu-latest").unwrap())
        );
    }

    #[test]
    fn noops_when_pool_absent() {
        let mut fm: Mapping = serde_yaml::from_str("name: x\ndescription: y").unwrap();
        let changed = apply_codemod(&mut fm, &CodemodContext {}).expect("apply");
        assert!(!changed);
    }

    #[test]
    fn translates_legacy_default_pool_for_non_onees() {
        let mut fm: Mapping =
            serde_yaml::from_str("name: x\ndescription: y\npool: AZS-1ES-L-MMS-ubuntu-22.04")
                .unwrap();
        let changed = apply_codemod(&mut fm, &CodemodContext {}).expect("apply");
        assert!(changed);
        assert_eq!(
            fm.get(Value::String("pool".into())).cloned(),
            Some(serde_yaml::from_str::<Value>("vmImage: ubuntu-latest").unwrap())
        );
    }

    #[test]
    fn keeps_legacy_default_pool_name_for_onees() {
        let mut fm: Mapping = serde_yaml::from_str(
            "name: x\ndescription: y\ntarget: 1es\npool: AZS-1ES-L-MMS-ubuntu-22.04",
        )
        .unwrap();
        let changed = apply_codemod(&mut fm, &CodemodContext {}).expect("apply");
        assert!(changed);
        assert_eq!(
            fm.get(Value::String("pool".into())).cloned(),
            Some(serde_yaml::from_str::<Value>("name: AZS-1ES-L-MMS-ubuntu-22.04").unwrap())
        );
    }
}
