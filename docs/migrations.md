# Front-matter Migrations

_Part of the [ado-aw documentation](../AGENTS.md)._

The `ado-aw` compiler maintains a versioned schema for the front-matter
grammar. When a breaking change is introduced (a renamed field, a removed
field, a type change), the compiler ships a **migration** that rewrites
older sources to the new shape automatically. Users see a warning during
compilation; their source files are updated in place; the build moves on.

This page is the reference for both users (what migrations mean for me)
and contributors (how to add one).

## How it works

### `schema-version` field

Every front-matter file carries an optional `schema-version: <u32>`
field. When the field is missing, it defaults to `1`. The compiler bumps
this in place after running migrations.

```yaml
---
schema-version: 1
name: my-agent
description: my agent
---
```

End users typically don't write this field by hand. The compiler keeps
it accurate when migrations apply.

### The compile flow

1. `ado-aw compile` reads the source `.md` and parses the front matter
   as an **untyped** `serde_yaml::Mapping`. This step never trips on
   removed/renamed fields.
2. The migration runner walks the registered chain from the source's
   `schema-version` up to the compiler's current version, applying each
   migration's transformation in order.
3. The compiler runs all the usual validation and codegen against the
   migrated, typed `FrontMatter`.
4. **Only on a fully successful compile**, the source `.md` is
   atomically rewritten with the migrated front matter and a warning
   prints to stderr. A failed compile leaves the source untouched.
5. The `.lock.yml` is written atomically last.

### What gets preserved on rewrite

- **Body markdown** is preserved byte-for-byte (everything after the
  closing `---`).
- **Front-matter key order** is preserved by `serde_yaml`'s
  insertion-ordered mapping.
- **Front-matter comments** are NOT preserved. `serde_yaml` round-trip
  drops them. The warning emitted on rewrite calls this out so it isn't
  a surprise. If you have important context in a front-matter comment,
  move it into the markdown body before running compile.
- **Quote and scalar styles** in YAML may be normalized. This is
  cosmetic.

### Atomicity and lost-update protection

The rewrite uses `tempfile + rename` for atomicity (no torn writes).
Before the rename, the runner re-reads the source and compares its
SHA-256 to the snapshot taken at parse time. If the file changed
between parse and rewrite, the runner aborts with a clear error
("source file ... changed during compilation; refusing to migrate")
rather than clobbering whoever wrote the file.

### `check` command behavior

`ado-aw check` exits non-zero when migrations are pending — there is no
opt-in flag and no warning-only mode. Rationale: compiled pipelines
download the **same** `ado-aw` version that produced them
(`src/data/base.yml`, `src/data/1es-base.yml`), so the in-pipeline
integrity check is internally consistent by construction. The only time
`check` sees a pending migration is when a developer runs a newer
`ado-aw` locally against an older source — exactly when we want to
fail loudly. The fix is `ado-aw compile`, which migrates the source in
place.

### `execute` command behavior

The Stage 3 executor runs migrations in memory only. It never rewrites
the source (the executor's working tree is not the source-of-truth
tree). When a migration applies, it logs a warning and continues.

## Adding a migration

You need a migration whenever you introduce a breaking change to the
front-matter grammar:

- Renaming a field
- Removing a field
- Changing a field's type or shape
- Adding a required field that didn't exist before
- Changing the meaning of an existing field

Non-breaking changes (adding an optional field, accepting a new
variant) do **not** need a migration.

### File layout

Migrations live in `src/compile/migrations/`:

```
src/compile/migrations/
├── mod.rs       # Framework + MIGRATIONS registry
├── helpers.rs   # take_key, insert_no_overwrite, rename_key, ConflictPolicy
├── 0001_engine_id_split.rs
├── 0002_permissions_field.rs
└── 0003_safeoutput_renames.rs
```

The filename prefix is the zero-padded `from_version`. Files sort
naturally in the directory listing in chain order.

### Anatomy of a migration

```rust
// src/compile/migrations/0001_engine_id_split.rs

use anyhow::Result;
use serde_yaml::Mapping;

use super::{ConflictPolicy, Migration, MigrationContext};

pub static MIGRATION: Migration = Migration {
    from_version: 1,
    to_version: 2,
    id: "engine_id_split",
    summary: "engine: <model> -> engine: { id: copilot, model: <model> }",
    introduced_in: "0.27.0",
    apply: apply_migration,
};

fn apply_migration(fm: &mut Mapping, _ctx: &MigrationContext) -> Result<()> {
    // ... your transformation ...
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upgrades_engine_string_to_object() {
        // before / after fixture pair
    }

    #[test]
    fn already_current_shape_is_preserved() {
        // defensive: every from_version=1 migration runs on every
        // unstamped source in the wild, including ones that were
        // authored *yesterday* against the current shape.
    }
}
```

### Registry append

Two edits in `src/compile/migrations/mod.rs`:

```rust
mod m0001_engine_id_split;  // <-- add module declaration

pub static MIGRATIONS: &[&'static Migration] = &[
    &m0001_engine_id_split::MIGRATION,  // <-- append at the end
];
```

`CURRENT_SCHEMA_VERSION` is computed automatically from the registry
length. Tests in `mod.rs` enforce that the registry is contiguous, that
migration ids are unique, and that filenames match `from_version`. A
malformed registry fails fast at runtime via `ensure!()`.

### Use the helpers

Migrations should prefer `helpers::*` over raw `Mapping` manipulation:

- `take_key(map, "old")` — remove and return.
- `insert_no_overwrite(map, "new", value)` — error on conflict.
- `rename_key(map, "old", "new", ConflictPolicy::Error)` — default
  policy is **error**, never silent overwrite.

Silent overwrite is almost always a bug when transforming user data.

### Defensive predicates for `from_version: 1`

Because the `schema-version` field doesn't exist in any source written
before this framework shipped, every existing file looks like v1 —
including files that were authored yesterday against the current
shape. Migrations from `from_version: 1` must be **shape-detecting and
conflict-aware**:

- If both old and new keys are present → error rather than silent
  overwrite (default `ConflictPolicy::Error`).
- If the old key has the new shape already → no-op (don't migrate).

Once we ship v2, this concern goes away for v2 → v3 etc. — v2 sources
have an explicit `schema-version: 2` stamp, so the framework only runs
migrations against actual v2 documents.

### Concurrent PRs

If two PRs each add a migration `N → N+1`, the second one to merge
must rebase. The rebase is mechanical:

1. Renumber the file prefix: `0003_…rs` → `0004_…rs`.
2. Bump `from_version` and `to_version` in the static.
3. Update any per-migration test fixtures that stamped the old
   version.
4. Re-append your migration to `MIGRATIONS` after the freshly merged
   one.

The contiguity tests fail loudly if a rebase leaves a gap, so this is
hard to get wrong silently.

### What if my change can't be expressed as a `Mapping` rewrite?

The migration `apply` function receives only the front-matter mapping.
It cannot inspect the markdown body, the file path, the lock file, or
git state. If your change requires that information, do not write a
migration that guesses. Instead, add a migration that **errors** with
an actionable "manual migration required: <instructions>" message so
the user knows exactly what to fix.

## Worked example: `engine_id_split`

This is the migration that would have caught the `0.17.0` breaking
change (`engine: <model-string>` → `engine: { id: copilot, model: <model> }`).
It's a complete file you could drop into `src/compile/migrations/`
and register today. Read it as a template for your own migration.

```rust
// src/compile/migrations/0001_engine_id_split.rs

//! engine: <model-string> -> engine: { id: copilot, model: <model> }
//!
//! Before 0.17.0 the `engine` field was a model name (e.g.
//! `engine: claude-opus-4.5`). The grammar changed to use engine
//! identifiers (`engine: copilot`), with the model nested in an object
//! form (`engine: { id: copilot, model: <name> }`).
//!
//! This migration detects the old shape and rewrites it.

use anyhow::{bail, Result};
use serde_yaml::{Mapping, Value};

use super::{Migration, MigrationContext};

pub static MIGRATION: Migration = Migration {
    from_version: 1,
    to_version: 2,
    id: "engine_id_split",
    summary: "engine: <model> -> engine: { id: copilot, model: <model> }",
    introduced_in: "0.27.0",
    apply: apply_migration,
};

/// Engine identifiers that are valid as the simple-form string. When
/// `engine` is a string equal to one of these, the source is already
/// using the current grammar and we leave it alone.
const KNOWN_ENGINE_IDS: &[&str] = &["copilot"];

fn apply_migration(fm: &mut Mapping, _ctx: &MigrationContext) -> Result<()> {
    let key = Value::String("engine".to_string());

    // No `engine` field at all — the front matter relies on the
    // default. Default is `copilot` in both old and new grammars, so
    // there's nothing to migrate.
    let Some(engine) = fm.get(&key) else {
        return Ok(());
    };

    match engine {
        // Already-object form. Either authored against the new
        // grammar from day one, or migrated by an earlier run. No-op.
        Value::Mapping(_) => Ok(()),

        // String form. Two cases:
        //   - The string is a known engine identifier (`copilot`):
        //     the source is already using the current simple form.
        //     No-op.
        //   - Anything else: the string is a *model name* from the
        //     old grammar. Wrap it in the object form.
        Value::String(s) => {
            if KNOWN_ENGINE_IDS.contains(&s.as_str()) {
                return Ok(());
            }
            let model = s.clone();
            let mut object = Mapping::new();
            object.insert(
                Value::String("id".to_string()),
                Value::String("copilot".to_string()),
            );
            object.insert(
                Value::String("model".to_string()),
                Value::String(model),
            );
            fm.insert(key, Value::Mapping(object));
            Ok(())
        }

        // Unexpected shape (number, bool, sequence, …). Refuse rather
        // than guess — the user needs to fix this by hand.
        other => bail!(
            "engine field has unexpected shape (expected string or mapping, \
             got {}); manual migration required",
            describe(other)
        ),
    }
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
        apply_migration(&mut m, &MigrationContext {}).expect("apply");
        m
    }

    fn run_err(input: &str) -> String {
        let mut m: Mapping = serde_yaml::from_str(input).unwrap();
        format!(
            "{}",
            apply_migration(&mut m, &MigrationContext {}).unwrap_err()
        )
    }

    #[test]
    fn rewrites_legacy_model_string_into_object_form() {
        let after = run("name: x\nengine: claude-opus-4.5\n");
        let engine = after
            .get(Value::String("engine".into()))
            .unwrap()
            .as_mapping()
            .expect("engine should now be a mapping");
        assert_eq!(
            engine.get(Value::String("id".into())),
            Some(&Value::String("copilot".into()))
        );
        assert_eq!(
            engine.get(Value::String("model".into())),
            Some(&Value::String("claude-opus-4.5".into()))
        );
    }

    #[test]
    fn already_current_simple_form_is_noop() {
        // Defensive: every from_version=1 migration must be safe on
        // sources that look like they're already current. Here the
        // user explicitly wrote the new simple form `engine: copilot`.
        let after = run("name: x\nengine: copilot\n");
        assert_eq!(
            after.get(Value::String("engine".into())),
            Some(&Value::String("copilot".into())),
            "string `copilot` should be left untouched"
        );
    }

    #[test]
    fn already_object_form_is_noop() {
        let input = "name: x\nengine:\n  id: copilot\n  model: claude-opus-4.7\n";
        let mut original: Mapping = serde_yaml::from_str(input).unwrap();
        let after = run(input);
        assert_eq!(
            after.get(Value::String("engine".into())),
            original
                .remove(Value::String("engine".into()))
                .as_ref()
        );
    }

    #[test]
    fn missing_engine_field_is_noop() {
        let after = run("name: x\ndescription: y\n");
        assert!(!after.contains_key(Value::String("engine".into())));
    }

    #[test]
    fn unexpected_engine_shape_is_rejected() {
        let err = run_err("name: x\nengine: 42\n");
        assert!(
            err.contains("manual migration required"),
            "expected actionable error, got: {}",
            err
        );
    }
}
```

### What this example illustrates

1. **The data model does the work.** `Migration` is a static —
   every field is set once at the top of the file. The runner reads
   `from_version`/`to_version` to walk the chain. `apply` is a plain
   `fn` pointer, so the registry is `&'static [&'static Migration]`
   — zero allocation, zero indirection beyond the function call.

2. **`Value` pattern-matching beats `if let` ladders.** The
   `match engine { Mapping(_) => …, String(s) => …, other => … }`
   shape is the natural way to express "I support these shapes;
   reject others." It catches numeric/boolean/null engines that
   someone might have written by mistake.

3. **Defensive predicates for `from_version: 1`.** Three of the five
   tests exercise no-op cases (already-object, already-simple
   `copilot`, missing field). These matter because **every existing
   `.md` file in the wild looks like v1** — they don't carry
   `schema-version`. The migration runs on all of them, so it has
   to be safe on already-current shapes. Once we ship v2 the
   defensiveness requirement drops for v2 → v3 migrations: v2
   sources have an explicit stamp.

4. **Hard error on unexpected shapes.** The `other => bail!(…)`
   arm is the "manual migration required" escape hatch. We don't
   try to guess what `engine: 42` means — we surface a clear error
   so the user fixes it by hand.

5. **Registering it is two lines** in `src/compile/migrations/mod.rs`:

   ```rust
   mod m0001_engine_id_split;

   pub static MIGRATIONS: &[&'static Migration] = &[
       &m0001_engine_id_split::MIGRATION,
   ];
   ```

   `CURRENT_SCHEMA_VERSION` automatically becomes 2. The
   registry-contiguity test, the filename-prefix test, and the
   id-uniqueness test all keep passing.

## Tests

The migration framework is covered by three layers of tests:

- **Unit tests** in `src/compile/migrations/{mod.rs,helpers.rs}`
  cover registry contiguity, helper edge cases, and individual
  migration apply functions.
- **White-box integration tests** in
  `src/compile/migration_integration_test.rs` exercise the rewrite
  path end-to-end (parse → migrate → compile → atomic source rewrite
  → lock-file write) using a stub migration registry injected via the
  crate-private `compile_pipeline_with_registry` and
  `parse_markdown_detailed_with_registry` hooks. They live inside
  `src/` because the production registry is empty and integration
  tests in `tests/` cannot link against crate internals.
- **Black-box CLI tests** in `tests/migration_tests.rs` spawn the
  compiled `ado-aw` binary as a subprocess and assert on the
  user-visible behavior of `compile` and `check` for sources with
  various `schema-version` shapes (future versions, non-integer values,
  healthy round-trip, etc.).

When you add a real migration, ship the per-migration before/after
fixtures alongside it in the migration's own file
(`src/compile/migrations/<NNNN>_<id>.rs`).

## See also

- [`docs/front-matter.md`](front-matter.md) — the full front-matter
  grammar (and where the `schema-version` field is documented for end
  users).
- [`docs/extending.md`](extending.md) — broader guidance for adding
  features to the compiler, including the requirement to add a
  migration alongside any breaking front-matter change.
