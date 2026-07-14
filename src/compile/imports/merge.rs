//! Consumer-wins front-matter merge for `imports:` (decision D9).
//!
//! Merges the front matter and body of resolved imported components into the
//! consumer workflow. Precedence is **consumer > later import > earlier
//! import**:
//!
//! * **Scalar / singleton keys** (`name`, `engine`, `target`, …): the
//!   highest-precedence explicit setter wins. No error.
//! * **Collection keys** (`tools`, `mcp-servers`, `safe-outputs`, `runtimes`,
//!   `env`): additive union by sub-key. A sub-key defined by **two different
//!   imports** is a hard error. The consumer may **configure** an imported
//!   `safe-outputs` tool (overlay non-executor config) but may **not** redefine
//!   its executor (`steps`/`env`/`inputs`/`run`/`entrypoint`) — that is a hard
//!   error.
//! * **Sequence keys** (`parameters`, `repos`, `variable-groups`): additive
//!   concatenation (imports first, then consumer).
//! * **Body**: imported bodies are concatenated in declaration order, then the
//!   consumer body.
//!
//! The merge runs only when the consumer declares `imports:`; with no imports
//! it is never invoked, so existing workflows are unaffected.
#![allow(dead_code)]

use std::path::Path;

use anyhow::{Context, Result};
use serde_yaml::{Mapping, Value};

use super::schema::apply_import_inputs;
use super::{ManifestFetcher, ResolvedImport, resolve_imports_with_repo_root};
use crate::compile::types::ImportEntry;

/// Front-matter keys whose values are mappings merged additively by sub-key.
const COLLECTION_MAP_KEYS: &[&str] = &["tools", "mcp-servers", "safe-outputs", "runtimes", "env"];

/// Front-matter keys whose values are sequences merged by concatenation.
const SEQUENCE_KEYS: &[&str] = &["parameters", "repos", "variable-groups"];

/// `safe-outputs` sub-keys that define a tool's executor. A consumer may
/// configure an imported tool but may not redefine these.
const EXECUTOR_KEYS: &[&str] = &["steps", "env", "inputs", "run", "entrypoint"];

/// Resolve the consumer's imports, apply their `import-schema` inputs, and
/// merge their front matter + body into `consumer_fm` / the returned body.
///
/// Returns the merged markdown body. `consumer_fm` is mutated in place and its
/// `imports:` key is removed (imports are consumed by this pass).
pub fn merge_imports(
    consumer_fm: &mut Mapping,
    consumer_body: &str,
    entries: &[ImportEntry],
    base_dir: &Path,
    repo_root: &Path,
    fetcher: &dyn ManifestFetcher,
) -> Result<String> {
    let resolved = resolve_imports_with_repo_root(entries, base_dir, repo_root, fetcher)?;
    merge_resolved(consumer_fm, consumer_body, &resolved)
}

/// Merge already-resolved imports (test-friendly seam that takes no fetcher).
pub fn merge_resolved(
    consumer_fm: &mut Mapping,
    consumer_body: &str,
    resolved: &[ResolvedImport],
) -> Result<String> {
    // Accumulate imported front matter in declaration order (import-vs-import
    // rules), then overlay the consumer on top.
    let mut acc = Mapping::new();
    let mut acc_provenance: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut body_parts: Vec<String> = Vec::new();

    for (idx, import) in resolved.iter().enumerate() {
        let (sub_fm, sub_body) =
            apply_import_inputs(&import.front_matter, &import.body, &import.entry.with)
                .with_context(|| {
                    format!(
                        "failed to apply import inputs for '{}'",
                        import.provenance.source
                    )
                })?;

        if let Value::Mapping(component_map) = &sub_fm {
            merge_import_into_acc(
                &mut acc,
                &mut acc_provenance,
                component_map,
                idx,
                &import.provenance.source,
            )?;
        }

        let trimmed = sub_body.trim();
        if !trimmed.is_empty() {
            body_parts.push(trimmed.to_string());
        }
    }

    // Overlay the consumer front matter on top of the accumulated imports.
    overlay_consumer(&mut acc, consumer_fm)?;

    // The merged mapping replaces the consumer mapping; drop the now-consumed
    // `imports` key.
    acc.remove(Value::String("imports".to_string()));
    *consumer_fm = acc;

    // Body: imported bodies (declaration order) then the consumer body.
    let consumer_trimmed = consumer_body.trim();
    if !consumer_trimmed.is_empty() {
        body_parts.push(consumer_trimmed.to_string());
    }
    Ok(body_parts.join("\n\n"))
}

/// Merge one import's mapping into the accumulator, enforcing import-vs-import
/// collision rules.
fn merge_import_into_acc(
    acc: &mut Mapping,
    provenance: &mut std::collections::HashMap<String, usize>,
    component: &Mapping,
    import_idx: usize,
    source: &str,
) -> Result<()> {
    for (key, value) in component {
        let key_str = match key.as_str() {
            Some(k) => k.to_string(),
            None => continue,
        };
        // `import-schema` is consumed by substitution and must never leak into
        // the merged workflow.
        if key_str == "import-schema" || key_str == "imports" {
            continue;
        }

        if is_collection_map_key(&key_str) {
            merge_map_key(
                acc,
                &key_str,
                value,
                MergeSide::Import {
                    idx: import_idx,
                    source,
                },
                provenance,
            )?;
        } else if is_sequence_key(&key_str) {
            concat_sequence(acc, &key_str, value);
        } else {
            // Scalar/singleton: later import wins over earlier.
            acc.insert(Value::String(key_str), value.clone());
        }
    }
    Ok(())
}

/// Overlay the consumer front matter on top of the accumulated imports.
fn overlay_consumer(acc: &mut Mapping, consumer: &Mapping) -> Result<()> {
    for (key, value) in consumer {
        let key_str = match key.as_str() {
            Some(k) => k.to_string(),
            None => continue,
        };
        if key_str == "imports" {
            continue;
        }

        if is_collection_map_key(&key_str) {
            merge_map_key(
                acc,
                &key_str,
                value,
                MergeSide::Consumer,
                &mut Default::default(),
            )?;
        } else if is_sequence_key(&key_str) {
            concat_sequence(acc, &key_str, value);
        } else {
            // Scalar/singleton: consumer wins.
            acc.insert(Value::String(key_str), value.clone());
        }
    }
    Ok(())
}

enum MergeSide<'a> {
    Import { idx: usize, source: &'a str },
    Consumer,
}

/// Merge a collection-map key (e.g. `tools`) into the accumulator, applying the
/// per-sub-key collision rules.
fn merge_map_key(
    acc: &mut Mapping,
    key: &str,
    incoming: &Value,
    side: MergeSide<'_>,
    provenance: &mut std::collections::HashMap<String, usize>,
) -> Result<()> {
    let Value::Mapping(incoming_map) = incoming else {
        // Non-mapping value under a collection key: treat as scalar overwrite.
        acc.insert(Value::String(key.to_string()), incoming.clone());
        return Ok(());
    };

    let entry = acc
        .entry(Value::String(key.to_string()))
        .or_insert_with(|| Value::Mapping(Mapping::new()));
    let Value::Mapping(existing) = entry else {
        // Existing non-mapping (unusual) — replace wholesale.
        *entry = incoming.clone();
        return Ok(());
    };

    for (sub_key, sub_val) in incoming_map {
        let sub_name = match sub_key.as_str() {
            Some(s) => s.to_string(),
            None => continue,
        };
        let prov_key = format!("{key}.{sub_name}");
        let already = existing.contains_key(sub_key);

        match &side {
            MergeSide::Import { idx, source } => {
                if already {
                    // Collision between two imports is a hard error.
                    let prev = provenance.get(&prov_key).copied();
                    if prev.is_some() && prev != Some(*idx) {
                        anyhow::bail!(
                            "import conflict: '{key}.{sub_name}' is defined by more than one \
                             imported component (latest from '{source}'). Imported \
                             {key} entries must have unique names."
                        );
                    }
                }
                existing.insert(sub_key.clone(), sub_val.clone());
                provenance.insert(prov_key, *idx);
            }
            MergeSide::Consumer => {
                if already && key == "safe-outputs" {
                    // Consumer may configure an imported tool but not redefine
                    // its executor.
                    configure_safe_output(existing, sub_key, sub_val, &sub_name)?;
                } else if already {
                    anyhow::bail!(
                        "import conflict: the consumer redefines '{key}.{sub_name}', which is \
                         already provided by an imported component. Collections merge \
                         additively; rename or remove the duplicate."
                    );
                } else {
                    existing.insert(sub_key.clone(), sub_val.clone());
                }
            }
        }
    }
    Ok(())
}

/// Overlay consumer configuration onto an imported `safe-outputs` tool without
/// allowing executor redefinition.
fn configure_safe_output(
    existing: &mut Mapping,
    sub_key: &Value,
    incoming: &Value,
    sub_name: &str,
) -> Result<()> {
    let existing_val = existing.get_mut(sub_key);
    match (existing_val, incoming) {
        (Some(Value::Mapping(existing_cfg)), Value::Mapping(incoming_cfg)) => {
            for (cfg_key, cfg_val) in incoming_cfg {
                if let Some(name) = cfg_key.as_str()
                    && EXECUTOR_KEYS.contains(&name)
                {
                    anyhow::bail!(
                        "import conflict: the consumer may configure the imported \
                         safe-output '{sub_name}' but not redefine its executor \
                         ('{name}' is executor-defining)."
                    );
                }
                existing_cfg.insert(cfg_key.clone(), cfg_val.clone());
            }
            Ok(())
        }
        // If the consumer provides a non-mapping config (e.g. `true`), overlay
        // it as the tool value.
        (Some(slot), _) => {
            *slot = incoming.clone();
            Ok(())
        }
        (None, _) => {
            existing.insert(sub_key.clone(), incoming.clone());
            Ok(())
        }
    }
}

/// Concatenate a sequence-valued key (imports first, then consumer).
fn concat_sequence(acc: &mut Mapping, key: &str, incoming: &Value) {
    let Value::Sequence(incoming_seq) = incoming else {
        acc.insert(Value::String(key.to_string()), incoming.clone());
        return;
    };
    let entry = acc
        .entry(Value::String(key.to_string()))
        .or_insert_with(|| Value::Sequence(Vec::new()));
    if let Value::Sequence(existing) = entry {
        existing.extend(incoming_seq.iter().cloned());
    } else {
        *entry = incoming.clone();
    }
}

fn is_collection_map_key(key: &str) -> bool {
    COLLECTION_MAP_KEYS.contains(&key)
}

fn is_sequence_key(key: &str) -> bool {
    SEQUENCE_KEYS.contains(&key)
}

#[cfg(test)]
mod tests {
    use super::super::ImportProvenance;
    use super::*;
    use crate::compile::types::ImportSource;

    fn ymap(yaml: &str) -> Mapping {
        match serde_yaml::from_str::<Value>(yaml).unwrap() {
            Value::Mapping(m) => m,
            _ => panic!("expected mapping"),
        }
    }

    fn resolved(fm_yaml: &str, body: &str) -> ResolvedImport {
        ResolvedImport {
            entry: ImportEntry {
                uses: "local.md".to_string(),
                with: serde_json::Map::new(),
                endpoint: None,
            },
            source: ImportSource::Local {
                path: "local.md".to_string(),
                section: None,
                optional: false,
            },
            front_matter: serde_yaml::from_str(fm_yaml).unwrap(),
            body: body.to_string(),
            provenance: ImportProvenance {
                source: "local.md".to_string(),
                sha: None,
                manifest_digest: "d".to_string(),
            },
        }
    }

    #[test]
    fn consumer_wins_for_scalars() {
        let mut consumer = ymap("engine: copilot\nname: consumer");
        let imports = vec![resolved("engine: claude\ntarget: 1es", "")];
        merge_resolved(&mut consumer, "", &imports).unwrap();
        assert_eq!(
            consumer[Value::String("engine".into())],
            Value::String("copilot".into())
        );
        // Import-only scalar is adopted.
        assert_eq!(
            consumer[Value::String("target".into())],
            Value::String("1es".into())
        );
    }

    #[test]
    fn later_import_wins_over_earlier_for_scalars() {
        let mut consumer = ymap("name: c");
        let imports = vec![resolved("engine: a", ""), resolved("engine: b", "")];
        merge_resolved(&mut consumer, "", &imports).unwrap();
        assert_eq!(
            consumer[Value::String("engine".into())],
            Value::String("b".into())
        );
    }

    #[test]
    fn collections_union_additively() {
        let mut consumer = ymap("tools:\n  bash: {}");
        let imports = vec![resolved("tools:\n  edit: {}", "")];
        merge_resolved(&mut consumer, "", &imports).unwrap();
        let tools = consumer[Value::String("tools".into())]
            .as_mapping()
            .unwrap();
        assert!(tools.contains_key(Value::String("bash".into())));
        assert!(tools.contains_key(Value::String("edit".into())));
    }

    #[test]
    fn import_vs_import_collection_collision_errors() {
        let mut consumer = ymap("name: c");
        let imports = vec![
            resolved("mcp-servers:\n  x:\n    url: a", ""),
            resolved("mcp-servers:\n  x:\n    url: b", ""),
        ];
        let err = merge_resolved(&mut consumer, "", &imports).unwrap_err();
        assert!(err.to_string().contains("more than one"), "{err}");
    }

    #[test]
    fn consumer_redefining_imported_tool_errors() {
        let mut consumer = ymap("tools:\n  edit: {}");
        let imports = vec![resolved("tools:\n  edit: {}", "")];
        let err = merge_resolved(&mut consumer, "", &imports).unwrap_err();
        assert!(err.to_string().contains("redefines"), "{err}");
    }

    #[test]
    fn consumer_may_configure_imported_safe_output() {
        let mut consumer = ymap("safe-outputs:\n  notify:\n    require-approval: true");
        let imports = vec![resolved(
            "safe-outputs:\n  notify:\n    run: node notify.js\n    max: 3",
            "",
        )];
        merge_resolved(&mut consumer, "", &imports).unwrap();
        let so = consumer[Value::String("safe-outputs".into())]
            .as_mapping()
            .unwrap();
        let notify = so[Value::String("notify".into())].as_mapping().unwrap();
        assert_eq!(
            notify[Value::String("require-approval".into())],
            Value::Bool(true)
        );
        // Imported executor config is preserved.
        assert_eq!(
            notify[Value::String("run".into())],
            Value::String("node notify.js".into())
        );
    }

    #[test]
    fn consumer_redefining_safe_output_executor_errors() {
        let mut consumer = ymap("safe-outputs:\n  notify:\n    run: evil.js");
        let imports = vec![resolved("safe-outputs:\n  notify:\n    run: notify.js", "")];
        let err = merge_resolved(&mut consumer, "", &imports).unwrap_err();
        assert!(err.to_string().contains("executor"), "{err}");
    }

    #[test]
    fn body_concatenated_imports_then_consumer() {
        let mut consumer = ymap("name: c");
        let imports = vec![resolved("name: i", "IMPORT BODY")];
        let body = merge_resolved(&mut consumer, "CONSUMER BODY", &imports).unwrap();
        assert_eq!(body, "IMPORT BODY\n\nCONSUMER BODY");
    }

    #[test]
    fn imports_key_removed_after_merge() {
        let mut consumer = ymap("imports:\n  - local.md\nname: c");
        merge_resolved(&mut consumer, "", &[]).unwrap();
        assert!(!consumer.contains_key(Value::String("imports".into())));
    }

    #[test]
    fn sequences_concatenated() {
        let mut consumer = ymap("parameters:\n  - name: p2");
        let imports = vec![resolved("parameters:\n  - name: p1", "")];
        merge_resolved(&mut consumer, "", &imports).unwrap();
        let params = consumer[Value::String("parameters".into())]
            .as_sequence()
            .unwrap();
        assert_eq!(params.len(), 2);
    }
}
