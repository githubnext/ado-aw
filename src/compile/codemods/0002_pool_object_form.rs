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
    mapped.insert(Value::String("name".to_string()), Value::String(name));
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
    fn rewrites_legacy_default_pool_to_name_object() {
        let mut fm: Mapping =
            serde_yaml::from_str("name: x\ndescription: y\npool: AZS-1ES-L-MMS-ubuntu-22.04")
                .unwrap();
        let changed = apply_codemod(&mut fm, &CodemodContext {}).expect("apply");
        assert!(changed);
        assert_eq!(
            fm.get(Value::String("pool".into())).cloned(),
            Some(serde_yaml::from_str::<Value>("name: AZS-1ES-L-MMS-ubuntu-22.04").unwrap())
        );
    }
}
