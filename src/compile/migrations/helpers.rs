//! Helpers for writing migrations against `serde_yaml::Mapping`.
//!
//! Migrations should prefer these over raw `Mapping` manipulation so that
//! conflicts (e.g. both old and new keys present) are surfaced rather than
//! silently overwritten.

use anyhow::{bail, Result};
use serde_yaml::{Mapping, Value};

/// Conflict policy used by [`rename_key`] when the destination key is
/// already present.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub enum ConflictPolicy {
    /// Default: error if `new` already exists when renaming `old → new`.
    Error,
    /// Keep the existing `new` value, drop `old`.
    PreferNew,
    /// Overwrite `new` with the value from `old`.
    PreferOld,
}

/// Remove and return the value at `key`, or `None` if absent.
#[allow(dead_code)]
pub fn take_key(m: &mut Mapping, key: &str) -> Option<Value> {
    m.remove(Value::String(key.to_string()))
}

/// Insert `value` at `key`, returning `Err` if the key already exists.
///
/// Prefer this over `Mapping::insert` in migrations: silent overwrite is
/// almost always a bug when transforming user data.
#[allow(dead_code)]
pub fn insert_no_overwrite(
    m: &mut Mapping,
    key: &str,
    value: Value,
) -> Result<()> {
    if m.contains_key(Value::String(key.to_string())) {
        bail!(
            "refusing to overwrite existing key `{}` in front matter",
            key
        );
    }
    m.insert(Value::String(key.to_string()), value);
    Ok(())
}

/// Rename `old` → `new` according to `policy`.
///
/// Returns `Ok(true)` when the rename happened (regardless of policy
/// branch), `Ok(false)` when `old` was absent (no-op).
#[allow(dead_code)]
pub fn rename_key(
    m: &mut Mapping,
    old: &str,
    new: &str,
    policy: ConflictPolicy,
) -> Result<bool> {
    let Some(old_value) = take_key(m, old) else {
        return Ok(false);
    };
    let new_present = m.contains_key(Value::String(new.to_string()));
    match (new_present, policy) {
        (false, _) => {
            m.insert(Value::String(new.to_string()), old_value);
            Ok(true)
        }
        (true, ConflictPolicy::Error) => {
            bail!(
                "refusing to rename `{}` -> `{}`: destination key already exists \
                 (set both old and new keys at once is ambiguous)",
                old,
                new
            );
        }
        (true, ConflictPolicy::PreferNew) => Ok(true),
        (true, ConflictPolicy::PreferOld) => {
            m.insert(Value::String(new.to_string()), old_value);
            Ok(true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map_with(pairs: &[(&str, &str)]) -> Mapping {
        let mut m = Mapping::new();
        for (k, v) in pairs {
            m.insert(
                Value::String((*k).to_string()),
                Value::String((*v).to_string()),
            );
        }
        m
    }

    #[test]
    fn take_key_present_returns_value_and_removes_it() {
        let mut m = map_with(&[("a", "1"), ("b", "2")]);
        let v = take_key(&mut m, "a").unwrap();
        assert_eq!(v, Value::String("1".to_string()));
        assert!(!m.contains_key(Value::String("a".to_string())));
        assert!(m.contains_key(Value::String("b".to_string())));
    }

    #[test]
    fn take_key_absent_returns_none() {
        let mut m = map_with(&[("a", "1")]);
        assert!(take_key(&mut m, "missing").is_none());
        assert!(m.contains_key(Value::String("a".to_string())));
    }

    #[test]
    fn insert_no_overwrite_inserts_when_absent() {
        let mut m = Mapping::new();
        insert_no_overwrite(&mut m, "k", Value::String("v".into())).unwrap();
        assert_eq!(
            m.get(Value::String("k".into())),
            Some(&Value::String("v".into()))
        );
    }

    #[test]
    fn insert_no_overwrite_errors_on_conflict() {
        let mut m = map_with(&[("k", "v1")]);
        let err = insert_no_overwrite(&mut m, "k", Value::String("v2".into()))
            .unwrap_err();
        assert!(
            format!("{}", err).contains("refusing to overwrite"),
            "unexpected error message: {}",
            err
        );
        assert_eq!(
            m.get(Value::String("k".into())),
            Some(&Value::String("v1".into()))
        );
    }

    #[test]
    fn rename_key_old_absent_is_noop() {
        let mut m = map_with(&[("b", "2")]);
        let renamed = rename_key(&mut m, "a", "z", ConflictPolicy::Error).unwrap();
        assert!(!renamed);
        assert!(!m.contains_key(Value::String("z".into())));
    }

    #[test]
    fn rename_key_new_absent_moves_value() {
        let mut m = map_with(&[("old", "v")]);
        let renamed = rename_key(&mut m, "old", "new", ConflictPolicy::Error).unwrap();
        assert!(renamed);
        assert!(!m.contains_key(Value::String("old".into())));
        assert_eq!(
            m.get(Value::String("new".into())),
            Some(&Value::String("v".into()))
        );
    }

    #[test]
    fn rename_key_new_present_with_error_policy_fails() {
        let mut m = map_with(&[("old", "v_old"), ("new", "v_new")]);
        let err = rename_key(&mut m, "old", "new", ConflictPolicy::Error).unwrap_err();
        assert!(
            format!("{}", err).contains("destination key already exists"),
            "unexpected error message: {}",
            err
        );
    }

    #[test]
    fn rename_key_new_present_with_prefer_new_drops_old() {
        let mut m = map_with(&[("old", "v_old"), ("new", "v_new")]);
        let renamed =
            rename_key(&mut m, "old", "new", ConflictPolicy::PreferNew).unwrap();
        assert!(renamed);
        assert!(!m.contains_key(Value::String("old".into())));
        assert_eq!(
            m.get(Value::String("new".into())),
            Some(&Value::String("v_new".into()))
        );
    }

    #[test]
    fn rename_key_new_present_with_prefer_old_overwrites() {
        let mut m = map_with(&[("old", "v_old"), ("new", "v_new")]);
        let renamed =
            rename_key(&mut m, "old", "new", ConflictPolicy::PreferOld).unwrap();
        assert!(renamed);
        assert!(!m.contains_key(Value::String("old".into())));
        assert_eq!(
            m.get(Value::String("new".into())),
            Some(&Value::String("v_old".into()))
        );
    }
}
