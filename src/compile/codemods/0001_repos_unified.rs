//! `repositories:` + `checkout:` → unified `repos:`
//!
//! Before this codemod, additional repository resources had to be
//! declared twice: once under `repositories:` (mirroring ADO's
//! `resources.repositories` schema) and again under `checkout:` (the
//! alias list deciding which to actually clone). This codemod folds
//! both into a single `repos:` block where each entry carries its own
//! `checkout: true|false` flag (defaulting to `true`).
//!
//! Conversion rules:
//! - Each `repositories:` entry becomes a `repos:` entry preserving
//!   `repository` (→ `alias`), `name`, `type`, and `ref`.
//! - Entries listed in `checkout:` keep `checkout: true` (the default,
//!   so the field is omitted on output).
//! - Entries NOT listed in `checkout:` get an explicit `checkout: false`.
//! - `checkout:` aliases that don't match any `repositories:` entry are
//!   rejected (the legacy compiler also rejected these via
//!   `validate_checkout_list`).
//! - Mixing the legacy fields with an already-present `repos:` is
//!   rejected — the user must pick one shape.
//! - Sources with neither legacy field are no-ops (idempotent).

use anyhow::{bail, Result};
use serde_yaml::{Mapping, Value};

use super::{take_key, Codemod, CodemodContext};

pub static CODEMOD: Codemod = Codemod {
    id: "repos_unified",
    summary: "repositories: + checkout: -> unified repos:",
    introduced_in: "0.28.0",
    apply: apply_codemod,
};

fn apply_codemod(fm: &mut Mapping, _ctx: &CodemodContext) -> Result<bool> {
    let has_repos = fm.contains_key(Value::String("repos".to_string()));
    let has_repositories = fm.contains_key(Value::String("repositories".to_string()));
    let has_checkout = fm.contains_key(Value::String("checkout".to_string()));

    // No legacy fields → already in the new shape (or doesn't use any
    // additional repos at all). No-op (idempotent).
    if !has_repositories && !has_checkout {
        return Ok(false);
    }

    // Mixing the legacy fields with the new `repos:` is ambiguous —
    // refuse rather than guess which one wins.
    if has_repos {
        bail!(
            "front matter has both the new `repos:` field and the legacy \
             `repositories:`/`checkout:` fields. Pick one: remove the \
             legacy fields to use `repos:`, or remove `repos:` to let \
             this codemod convert the legacy fields."
        );
    }

    // `checkout:` without any `repositories:` is incoherent — the
    // aliases would have nothing to refer to. The legacy compiler
    // would have failed `validate_checkout_list` for the same reason.
    if has_checkout && !has_repositories {
        bail!(
            "front matter has `checkout:` but no `repositories:`. \
             Either remove `checkout:` or add the corresponding \
             `repositories:` entries (then re-run compile to migrate \
             to `repos:`)."
        );
    }

    let repositories = take_key(fm, "repositories").expect("checked above");
    let checkout = take_key(fm, "checkout");

    let repositories_seq = match repositories {
        Value::Sequence(s) => s,
        Value::Null => Vec::new(),
        other => bail!(
            "front matter `repositories:` must be a sequence, got {}",
            describe(&other)
        ),
    };

    // Collect the checkout alias allow-list. Order doesn't matter for
    // membership; we preserve `repositories:` order in the output.
    let checkout_aliases: Vec<String> = match checkout {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Sequence(s)) => {
            let mut out = Vec::with_capacity(s.len());
            for v in s {
                match v {
                    Value::String(name) => out.push(name),
                    other => bail!(
                        "front matter `checkout:` entries must be strings, got {}",
                        describe(&other)
                    ),
                }
            }
            out
        }
        Some(other) => bail!(
            "front matter `checkout:` must be a sequence of strings, got {}",
            describe(&other)
        ),
    };

    // Trivially-empty sources: an empty `repositories:` (and either no
    // `checkout:` or an empty one) carries no semantic content. We
    // already removed the vacuous keys from the mapping above, but
    // report this as a no-op so the caller doesn't surface a
    // "deprecated shapes" warning or rewrite the file just to drop
    // empty stubs.
    if repositories_seq.is_empty() && checkout_aliases.is_empty() {
        return Ok(false);
    }

    // Track which checkout aliases we've matched so we can flag
    // dangling references (alias listed in checkout but absent from
    // repositories).
    let mut matched: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::new();

    let mut repos_seq: Vec<Value> = Vec::with_capacity(repositories_seq.len());
    for repo in repositories_seq {
        let mut repo_map = match repo {
            Value::Mapping(m) => m,
            other => bail!(
                "front matter `repositories:` entries must be mappings, got {}",
                describe(&other)
            ),
        };

        // The legacy `repository:` key becomes the new `alias:` key.
        let alias_value = repo_map.remove(Value::String("repository".to_string()));
        let alias_str = match alias_value.as_ref() {
            Some(Value::String(s)) => Some(s.clone()),
            Some(other) => bail!(
                "front matter `repositories:` entry has non-string `repository:` field ({}); \
                 manual migration required",
                describe(other)
            ),
            None => None,
        };

        // Build the new entry. Preserve insertion order:
        // alias, type, name, ref, checkout.
        let mut new_entry = Mapping::new();
        if let Some(alias) = alias_str.as_deref() {
            new_entry.insert(
                Value::String("alias".to_string()),
                Value::String(alias.to_string()),
            );
        }
        if let Some(v) = repo_map.remove(Value::String("type".to_string())) {
            new_entry.insert(Value::String("type".to_string()), v);
        }
        if let Some(v) = repo_map.remove(Value::String("name".to_string())) {
            new_entry.insert(Value::String("name".to_string()), v);
        } else {
            bail!(
                "front matter `repositories:` entry is missing the required `name:` field; \
                 manual migration required"
            );
        }
        if let Some(v) = repo_map.remove(Value::String("ref".to_string())) {
            new_entry.insert(Value::String("ref".to_string()), v);
        }
        // Carry over any unknown keys verbatim so the codemod is
        // forward-compatible with future ADO `resources.repositories`
        // fields we don't yet model.
        for (k, v) in repo_map {
            new_entry.insert(k, v);
        }

        // Determine checkout flag. If `checkout:` was absent entirely,
        // legacy semantics treat all repositories as resource-only
        // (the agent job didn't clone any). If `checkout:` was
        // present, only listed aliases get cloned.
        let do_checkout = if !has_checkout {
            // Legacy: no `checkout:` block at all means nothing was
            // cloned by the agent.
            false
        } else if let Some(alias) = alias_str.as_deref() {
            let listed = checkout_aliases.iter().any(|a| a == alias);
            if listed {
                matched.insert(alias.to_string());
            }
            listed
        } else {
            // Anonymous entry (no `repository:` alias) cannot be
            // referenced from `checkout:` — treat as resource-only.
            false
        };

        // Only emit the `checkout` field when it deviates from the
        // default of `true`. Keeps rewritten output compact.
        if !do_checkout {
            new_entry.insert(
                Value::String("checkout".to_string()),
                Value::Bool(false),
            );
        }

        repos_seq.push(Value::Mapping(new_entry));
    }

    // Surface dangling checkout aliases (listed but no matching repo).
    for alias in &checkout_aliases {
        if !matched.contains(alias) {
            bail!(
                "front matter `checkout:` references alias `{}` that does not appear \
                 in `repositories:`; manual migration required",
                alias
            );
        }
    }

    // Insert `repos:` only when we actually have entries; an empty
    // `repositories:` should not produce an empty `repos:` block.
    if !repos_seq.is_empty() {
        fm.insert(
            Value::String("repos".to_string()),
            Value::Sequence(repos_seq),
        );
    }

    Ok(true)
}

fn describe(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Sequence(_) => "sequence",
        Value::Mapping(_) => "mapping",
        Value::Tagged(_) => "tagged",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(input: &str) -> Mapping {
        let mut m: Mapping = serde_yaml::from_str(input).unwrap();
        let changed = apply_codemod(&mut m, &CodemodContext {}).expect("apply");
        assert!(changed, "expected codemod to fire on input:\n{}", input);
        m
    }

    fn run_noop(input: &str) -> Mapping {
        let mut m: Mapping = serde_yaml::from_str(input).unwrap();
        let changed = apply_codemod(&mut m, &CodemodContext {}).expect("apply");
        assert!(!changed, "expected codemod to be a no-op on input:\n{}", input);
        m
    }

    fn run_err(input: &str) -> String {
        let mut m: Mapping = serde_yaml::from_str(input).unwrap();
        format!(
            "{}",
            apply_codemod(&mut m, &CodemodContext {}).unwrap_err()
        )
    }

    fn repos(m: &Mapping) -> &Vec<Value> {
        m.get(Value::String("repos".into()))
            .expect("repos key")
            .as_sequence()
            .expect("repos sequence")
    }

    #[test]
    fn converts_full_legacy_block_with_checkout_subset() {
        let after = run(
            "name: x\n\
             repositories:\n\
             - repository: tools\n  type: git\n  name: my-org/tools\n\
             - repository: schemas\n  type: git\n  name: my-org/schemas\n\
             - repository: docs\n  type: git\n  name: my-org/docs\n\
             checkout:\n- tools\n- schemas\n",
        );
        // Legacy keys removed
        assert!(!after.contains_key(Value::String("repositories".into())));
        assert!(!after.contains_key(Value::String("checkout".into())));

        let r = repos(&after);
        assert_eq!(r.len(), 3);

        let r0 = r[0].as_mapping().unwrap();
        assert_eq!(r0.get(Value::String("alias".into())).unwrap().as_str(), Some("tools"));
        assert_eq!(r0.get(Value::String("name".into())).unwrap().as_str(), Some("my-org/tools"));
        assert_eq!(r0.get(Value::String("type".into())).unwrap().as_str(), Some("git"));
        // checked out -> default, no explicit `checkout:` key.
        assert!(r0.get(Value::String("checkout".into())).is_none());

        let r2 = r[2].as_mapping().unwrap();
        // `docs` is NOT in checkout list -> explicit `checkout: false`.
        assert_eq!(
            r2.get(Value::String("checkout".into())),
            Some(&Value::Bool(false))
        );
    }

    #[test]
    fn converts_repositories_only_to_resource_only_entries() {
        // `repositories:` without `checkout:` means no entry was
        // cloned by the agent in the legacy semantics.
        let after = run(
            "name: x\n\
             repositories:\n\
             - repository: tpl\n  type: git\n  name: org/tpl\n",
        );
        let r = repos(&after);
        assert_eq!(r.len(), 1);
        assert_eq!(
            r[0].as_mapping().unwrap().get(Value::String("checkout".into())),
            Some(&Value::Bool(false)),
            "without an explicit checkout list, repos default to resource-only"
        );
    }

    #[test]
    fn preserves_ref_field() {
        let after = run(
            "name: x\n\
             repositories:\n\
             - repository: docs\n  type: git\n  name: org/docs\n  ref: refs/heads/release/2.x\n\
             checkout: [docs]\n",
        );
        let r = repos(&after);
        let entry = r[0].as_mapping().unwrap();
        assert_eq!(
            entry.get(Value::String("ref".into())).unwrap().as_str(),
            Some("refs/heads/release/2.x")
        );
    }

    #[test]
    fn rejects_mixing_repos_and_legacy_fields() {
        let err = run_err(
            "name: x\n\
             repos:\n- org/foo\n\
             repositories:\n- repository: bar\n  type: git\n  name: org/bar\n",
        );
        assert!(err.contains("Pick one"), "got: {}", err);
    }

    #[test]
    fn rejects_checkout_without_repositories() {
        let err = run_err("name: x\ncheckout: [foo]\n");
        assert!(err.contains("`checkout:` but no `repositories:`"), "got: {}", err);
    }

    #[test]
    fn rejects_dangling_checkout_alias() {
        let err = run_err(
            "name: x\n\
             repositories:\n- repository: a\n  type: git\n  name: org/a\n\
             checkout: [b]\n",
        );
        assert!(err.contains("does not appear in `repositories:`"), "got: {}", err);
    }

    #[test]
    fn no_legacy_fields_is_noop() {
        let after = run_noop("name: x\ndescription: y\n");
        assert!(!after.contains_key(Value::String("repos".into())));
        assert!(!after.contains_key(Value::String("repositories".into())));
        assert!(!after.contains_key(Value::String("checkout".into())));
    }

    #[test]
    fn already_using_repos_alone_is_noop() {
        // Idempotency: a file with only `repos:` (no legacy fields) is
        // a no-op.
        let after = run_noop(
            "name: x\nrepos:\n- my-org/tools\n",
        );
        let r = repos(&after);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].as_str(), Some("my-org/tools"));
    }

    #[test]
    fn empty_repositories_sequence_does_not_emit_repos_key() {
        // A trivially-empty `repositories: []` carries no semantic
        // content — the codemod cleans up the stub key but reports
        // changed=false so the caller doesn't surface a rewrite
        // warning.
        let after = run_noop("name: x\nrepositories: []\n");
        assert!(!after.contains_key(Value::String("repos".into())));
    }

    #[test]
    fn rejects_non_mapping_repository_entry() {
        let err = run_err(
            "name: x\nrepositories:\n- a-string-not-a-mapping\n",
        );
        assert!(err.contains("must be mappings"), "got: {}", err);
    }

    #[test]
    fn carries_over_unknown_repository_keys() {
        // Forward-compat: don't drop fields we don't yet model.
        let after = run(
            "name: x\n\
             repositories:\n- repository: a\n  type: git\n  name: org/a\n  trigger: none\n\
             checkout: [a]\n",
        );
        let r = repos(&after);
        let entry = r[0].as_mapping().unwrap();
        assert_eq!(
            entry.get(Value::String("trigger".into())).unwrap().as_str(),
            Some("none")
        );
    }

    #[test]
    fn idempotent_when_run_twice() {
        // Critical codemod invariant: running twice produces the same
        // final state as running once.
        let mut m: Mapping = serde_yaml::from_str(
            "name: x\n\
             repositories:\n- repository: a\n  type: git\n  name: org/a\n\
             checkout: [a]\n",
        )
        .unwrap();
        let first = apply_codemod(&mut m, &CodemodContext {}).expect("first");
        assert!(first, "first run should fire");
        let snapshot = m.clone();
        let second = apply_codemod(&mut m, &CodemodContext {}).expect("second");
        assert!(!second, "second run should be a no-op");
        assert_eq!(m, snapshot, "second run must not mutate");
    }
}
