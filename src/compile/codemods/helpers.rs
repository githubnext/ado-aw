//! Helpers for writing codemods against `serde_yaml::Mapping`.
//!
//! Codemods should prefer these over raw `Mapping` manipulation so that
//! conflicts (e.g. both old and new keys present) are surfaced rather than
//! silently overwritten.

use anyhow::{Result, bail};
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
pub fn take_key(m: &mut Mapping, key: &str) -> Option<Value> {
    m.remove(Value::String(key.to_string()))
}

/// Insert `value` at `key`, returning `Err` if the key already exists.
///
/// Prefer this over `Mapping::insert` in codemods: silent overwrite is
/// almost always a bug when transforming user data.
#[allow(dead_code)]
pub fn insert_no_overwrite(m: &mut Mapping, key: &str, value: Value) -> Result<()> {
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
/// Returns `Ok(true)` when the mapping was mutated in any way:
///
/// - the `old` key was moved to `new` (the typical rename), OR
/// - both keys were present with [`ConflictPolicy::PreferOld`] and
///   `new` was overwritten, OR
/// - both keys were present with [`ConflictPolicy::PreferNew`] and
///   the stale `old` key was dropped (note: the `new` value did not
///   change, but the mapping still mutated — codemod authors using
///   `PreferNew` should be aware that `Ok(true)` here means
///   "cleaned up a remnant," not "migrated semantic content").
///
/// Returns `Ok(false)` when `old` was absent (no-op).
///
/// The mapping is left **unchanged** on the error path. Callers can
/// rely on this invariant when chaining codemods: a failed rename
/// won't leave the mapping in a half-mutated state for the next call
/// to inspect.
#[allow(dead_code)]
pub fn rename_key(m: &mut Mapping, old: &str, new: &str, policy: ConflictPolicy) -> Result<bool> {
    let old_key = Value::String(old.to_string());
    let new_key = Value::String(new.to_string());

    if !m.contains_key(&old_key) {
        return Ok(false);
    }
    let new_present = m.contains_key(&new_key);

    match (new_present, policy) {
        (true, ConflictPolicy::Error) => {
            // Surface the conflict without mutating the mapping.
            bail!(
                "refusing to rename `{}` -> `{}`: destination key already exists \
                 (setting both old and new keys at once is ambiguous)",
                old,
                new
            );
        }
        (false, _) | (true, ConflictPolicy::PreferOld) => {
            // Move old -> new, replacing any existing new value.
            let old_value = m.remove(&old_key).expect("old key checked above");
            m.insert(new_key, old_value);
            Ok(true)
        }
        (true, ConflictPolicy::PreferNew) => {
            // Drop old, keep new.
            m.remove(&old_key);
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
        let err = insert_no_overwrite(&mut m, "k", Value::String("v2".into())).unwrap_err();
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
        // Both keys must remain intact — the helper guarantees the
        // mapping is unchanged on the error path.
        assert_eq!(
            m.get(Value::String("old".into())),
            Some(&Value::String("v_old".into())),
            "old key must be preserved on error"
        );
        assert_eq!(
            m.get(Value::String("new".into())),
            Some(&Value::String("v_new".into())),
            "new key must be preserved on error"
        );
    }

    #[test]
    fn rename_key_new_present_with_prefer_new_drops_old() {
        let mut m = map_with(&[("old", "v_old"), ("new", "v_new")]);
        let renamed = rename_key(&mut m, "old", "new", ConflictPolicy::PreferNew).unwrap();
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
        let renamed = rename_key(&mut m, "old", "new", ConflictPolicy::PreferOld).unwrap();
        assert!(renamed);
        assert!(!m.contains_key(Value::String("old".into())));
        assert_eq!(
            m.get(Value::String("new".into())),
            Some(&Value::String("v_old".into()))
        );
    }
}
