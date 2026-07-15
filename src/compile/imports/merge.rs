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
/// merge their front matter + body into `consumer_fm` / the returned bodies.
///
/// Returns `(imported_body, combined_body)`:
/// - `imported_body` is the substituted, joined bodies of the imported
///   components (declaration order), with the consumer body NOT appended.
///   This is inlined into the agent prompt at compile time because imported
///   component bodies are only substituted here — they cannot be delivered by
///   the default runtime-import path (which reads the consumer's own source).
/// - `combined_body` additionally appends the consumer body, and is what
///   `inlined-imports: true` folds into the compiled YAML.
///
/// `consumer_fm` is mutated in place and its `imports:` key is removed (imports
/// are consumed by this pass).
pub async fn merge_imports(
    consumer_fm: &mut Mapping,
    consumer_body: &str,
    entries: &[ImportEntry],
    base_dir: &Path,
    repo_root: &Path,
    fetcher: &dyn ManifestFetcher,
) -> Result<(String, String)> {
    let resolved = resolve_imports_with_repo_root(entries, base_dir, repo_root, fetcher).await?;
    let imported_body = merge_resolved_imported_body(consumer_fm, &resolved)?;
    let combined_body = join_bodies(&imported_body, consumer_body);
    Ok((imported_body, combined_body))
}

/// Join the imported-body prefix with the consumer body using the same
/// `\n\n` separator as the merge accumulator, tolerating either side being
/// empty.
fn join_bodies(imported_body: &str, consumer_body: &str) -> String {
    let consumer_trimmed = consumer_body.trim();
    match (imported_body.is_empty(), consumer_trimmed.is_empty()) {
        (true, _) => consumer_trimmed.to_string(),
        (false, true) => imported_body.to_string(),
        (false, false) => format!("{imported_body}\n\n{consumer_trimmed}"),
    }
}

/// Merge already-resolved imports (test-friendly seam that takes no fetcher).
///
/// Returns the combined body (imported bodies in declaration order, then the
/// consumer body). Front matter is merged into `consumer_fm`.
pub fn merge_resolved(
    consumer_fm: &mut Mapping,
    consumer_body: &str,
    resolved: &[ResolvedImport],
) -> Result<String> {
    let imported_body = merge_resolved_imported_body(consumer_fm, resolved)?;
    Ok(join_bodies(&imported_body, consumer_body))
}

/// Merge resolved imports' front matter into `consumer_fm` and return the
/// substituted, joined **imported** bodies (declaration order) — the consumer
/// body is NOT appended here (see [`merge_imports`] for why the imported body
/// is tracked separately from the consumer body).
pub fn merge_resolved_imported_body(
    consumer_fm: &mut Mapping,
    resolved: &[ResolvedImport],
) -> Result<String> {
    // Accumulate imported front matter in declaration order (import-vs-import
    // rules), then overlay the consumer on top.
    let mut acc = Mapping::new();
    let mut acc_provenance: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut body_parts: Vec<String> = Vec::new();

    for (idx, import) in resolved.iter().enumerate() {
        let (mut sub_fm, sub_body) =
            apply_import_inputs(&import.front_matter, &import.body, &import.entry.with)
                .with_context(|| {
                    format!(
                        "failed to apply import inputs for '{}'",
                        import.provenance.source
                    )
                })?;

        // Stamp compile-time component provenance (source / sha / manifest
        // digest + resolved repo-type/service-connection from the typed
        // endpoint) onto this import's custom safe-output tools, so the runtime
        // executor job can check the component repo out at the pinned SHA. Only
        // remote imports have a repo to check out.
        stamp_component_provenance(&mut sub_fm, import);

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

    Ok(body_parts.join("\n\n"))
}

/// Stamp compiler-owned component provenance onto the custom safe-output tools
/// (`safe-outputs.scripts.*` / `safe-outputs.jobs.*`) declared by an import, so
/// the runtime executor job can check the component repository out at the pinned
/// commit — while ensuring the compiler **fully owns** these keys.
///
/// The `component-*` keys are compiler-owned provenance. For **every** imported
/// component (local or remote) any author-provided `component-*` value is first
/// **stripped**, so a component cannot spoof its own checkout (e.g. inject a
/// `component-endpoint` service connection or redirect `component-source`).
/// Then, for **remote** imports only, the compiler-resolved values are stamped:
/// `component-source` (owner/repo/path), `component-sha`, `manifest-digest`,
/// `component-repo-type` (`git` | `github` | `githubenterprise`), and — when the
/// endpoint names a service connection — `component-endpoint`. Local imports
/// (whose components live in the consumer repo's own checkout) end up with no
/// provenance keys and thus synthesize no separate checkout resource.
fn stamp_component_provenance(component_fm: &mut Value, import: &ResolvedImport) {
    use crate::compile::types::ImportSource;

    /// Compiler-owned provenance keys — never author-settable.
    const PROVENANCE_KEYS: [&str; 5] = [
        "component-source",
        "component-sha",
        "manifest-digest",
        "component-repo-type",
        "component-endpoint",
    ];

    let Value::Mapping(fm_map) = component_fm else {
        return;
    };
    let Some(Value::Mapping(safe_outputs)) = fm_map.get_mut("safe-outputs") else {
        return;
    };

    // Remote imports carry a repo + pinned SHA to check out; local imports do
    // not (their components live in the consumer's own checkout).
    let remote_sha = match &import.source {
        ImportSource::Remote(_) => import.provenance.sha.as_deref(),
        _ => None,
    };
    let (repo_type, endpoint_name) =
        crate::compile::imports::alias::endpoint_repo_type_and_connection(
            import.entry.endpoint.as_ref(),
        );

    for section in ["scripts", "jobs"] {
        let Some(Value::Mapping(tools)) = safe_outputs.get_mut(section) else {
            continue;
        };
        for (_tool_name, tool_cfg) in tools.iter_mut() {
            let Value::Mapping(cfg) = tool_cfg else {
                continue;
            };

            // Strip any author-provided provenance first (compiler fully owns
            // these keys).
            for key in PROVENANCE_KEYS {
                cfg.remove(Value::String(key.to_string()));
            }

            // Stamp compiler-resolved provenance for remote imports only.
            if let Some(sha) = remote_sha {
                cfg.insert(
                    Value::String("component-source".into()),
                    Value::String(import.provenance.source.clone()),
                );
                cfg.insert(
                    Value::String("component-sha".into()),
                    Value::String(sha.to_string()),
                );
                cfg.insert(
                    Value::String("manifest-digest".into()),
                    Value::String(import.provenance.manifest_digest.clone()),
                );
                cfg.insert(
                    Value::String("component-repo-type".into()),
                    Value::String(repo_type.to_string()),
                );
                if let Some(name) = &endpoint_name {
                    cfg.insert(
                        Value::String("component-endpoint".into()),
                        Value::String(name.clone()),
                    );
                }
            }
        }
    }
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

    const REMOTE_SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    fn remote_resolved(
        fm_yaml: &str,
        endpoint: Option<crate::compile::types::ImportEndpoint>,
    ) -> ResolvedImport {
        use crate::compile::types::ParsedImportSpec;
        use crate::secure::CommitSha;
        ResolvedImport {
            entry: ImportEntry {
                uses: format!("octo/repo/notify.md@{REMOTE_SHA}"),
                with: serde_json::Map::new(),
                endpoint: endpoint.clone(),
            },
            source: ImportSource::Remote(ParsedImportSpec {
                owner: "octo".to_string(),
                repo: "repo".to_string(),
                path: "notify.md".to_string(),
                sha: CommitSha::parse(REMOTE_SHA).unwrap(),
                section: None,
                optional: false,
                endpoint,
            }),
            front_matter: serde_yaml::from_str(fm_yaml).unwrap(),
            body: String::new(),
            provenance: ImportProvenance {
                source: "octo/repo/notify.md".to_string(),
                sha: Some(REMOTE_SHA.to_string()),
                manifest_digest: "digest123".to_string(),
            },
        }
    }

    fn scripts_notify_tool(consumer: &Mapping) -> &Mapping {
        consumer
            .get(Value::String("safe-outputs".into()))
            .and_then(Value::as_mapping)
            .and_then(|so| so.get(Value::String("scripts".into())))
            .and_then(Value::as_mapping)
            .and_then(|s| s.get(Value::String("notify".into())))
            .and_then(Value::as_mapping)
            .expect("safe-outputs.scripts.notify present")
    }

    fn tool_str<'a>(tool: &'a Mapping, key: &str) -> Option<&'a str> {
        tool.get(Value::String(key.into())).and_then(Value::as_str)
    }

    #[test]
    fn remote_component_gets_provenance_and_endpoint_stamped() {
        use crate::compile::types::ImportEndpoint;
        let mut consumer = ymap("name: consumer");
        let import = remote_resolved(
            "safe-outputs:\n  scripts:\n    notify:\n      run: node n.js\n",
            Some(ImportEndpoint::GitHub {
                name: "gh-conn".to_string(),
            }),
        );
        merge_resolved_imported_body(&mut consumer, &[import]).unwrap();

        let notify = scripts_notify_tool(&consumer);
        assert_eq!(tool_str(notify, "component-source"), Some("octo/repo/notify.md"));
        assert_eq!(tool_str(notify, "component-sha"), Some(REMOTE_SHA));
        assert_eq!(tool_str(notify, "manifest-digest"), Some("digest123"));
        assert_eq!(tool_str(notify, "component-repo-type"), Some("github"));
        assert_eq!(tool_str(notify, "component-endpoint"), Some("gh-conn"));
    }

    #[test]
    fn remote_same_org_azure_component_stamps_git_without_endpoint() {
        let mut consumer = ymap("name: consumer");
        // Endpoint-less remote import => same-org Azure Repos (`git`, no conn).
        let import = remote_resolved(
            "safe-outputs:\n  jobs:\n    notify:\n      steps:\n        - bash: echo hi\n",
            None,
        );
        merge_resolved_imported_body(&mut consumer, &[import]).unwrap();

        let notify = consumer
            .get(Value::String("safe-outputs".into()))
            .and_then(Value::as_mapping)
            .and_then(|so| so.get(Value::String("jobs".into())))
            .and_then(Value::as_mapping)
            .and_then(|j| j.get(Value::String("notify".into())))
            .and_then(Value::as_mapping)
            .expect("safe-outputs.jobs.notify present");
        assert_eq!(tool_str(notify, "component-repo-type"), Some("git"));
        assert_eq!(tool_str(notify, "component-endpoint"), None);
        assert_eq!(tool_str(notify, "component-sha"), Some(REMOTE_SHA));
    }

    #[test]
    fn local_component_is_not_stamped() {
        let mut consumer = ymap("name: consumer");
        let import = resolved(
            "safe-outputs:\n  scripts:\n    notify:\n      run: node n.js\n",
            "",
        );
        merge_resolved_imported_body(&mut consumer, &[import]).unwrap();

        let notify = scripts_notify_tool(&consumer);
        assert_eq!(tool_str(notify, "component-source"), None);
        assert_eq!(tool_str(notify, "component-repo-type"), None);
    }

    #[test]
    fn component_cannot_spoof_provenance_keys() {
        // A component authoring compiler-owned provenance keys into its own
        // front matter must NOT influence the checkout. For a same-org
        // (endpoint-less) remote import the compiler resolves no service
        // connection, so a pre-set `component-endpoint` must be stripped
        // (compiler fully owns the key). A spoofed `component-source` must be
        // overwritten with the real source.
        let mut consumer = ymap("name: consumer");
        let import = remote_resolved(
            "safe-outputs:\n  scripts:\n    notify:\n      run: node n.js\n      \
             component-endpoint: attacker-conn\n      component-source: evil/repo/x.md\n",
            None,
        );
        merge_resolved_imported_body(&mut consumer, &[import]).unwrap();

        let notify = scripts_notify_tool(&consumer);
        // Spoofed endpoint stripped (same-org => no connection).
        assert_eq!(tool_str(notify, "component-endpoint"), None);
        // Spoofed source overwritten with the compiler-resolved provenance.
        assert_eq!(tool_str(notify, "component-source"), Some("octo/repo/notify.md"));
        assert_eq!(tool_str(notify, "component-repo-type"), Some("git"));
    }

    #[test]
    fn remote_endpoint_overwrites_author_provided_component_endpoint() {
        use crate::compile::types::ImportEndpoint;
        // When the import DOES resolve a connection, the compiler value wins
        // over any author-provided one.
        let mut consumer = ymap("name: consumer");
        let import = remote_resolved(
            "safe-outputs:\n  scripts:\n    notify:\n      run: node n.js\n      \
             component-endpoint: attacker-conn\n",
            Some(ImportEndpoint::GitHub {
                name: "real-conn".to_string(),
            }),
        );
        merge_resolved_imported_body(&mut consumer, &[import]).unwrap();

        let notify = scripts_notify_tool(&consumer);
        assert_eq!(tool_str(notify, "component-endpoint"), Some("real-conn"));
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
    fn imported_body_and_combined_body_split_correctly() {
        // merge_resolved_imported_body returns ONLY the imported bodies; the
        // combined form (via merge_resolved) additionally appends the consumer
        // body. Imports-first ordering.
        let mut fm = ymap("name: consumer");
        let imports = vec![resolved("{}", "Import A."), resolved("{}", "Import B.")];
        let imported = merge_resolved_imported_body(&mut fm, &imports).unwrap();
        assert_eq!(imported, "Import A.\n\nImport B.");

        let mut fm2 = ymap("name: consumer");
        let combined = merge_resolved(&mut fm2, "Consumer body.", &imports).unwrap();
        assert_eq!(combined, "Import A.\n\nImport B.\n\nConsumer body.");
    }

    #[test]
    fn imported_body_empty_when_no_import_bodies() {
        let mut fm = ymap("name: consumer");
        let imports = vec![resolved("tools:\n  edit: {}", "")];
        let imported = merge_resolved_imported_body(&mut fm, &imports).unwrap();
        assert_eq!(imported, "");
        // Combined with a consumer body yields just the consumer body.
        let mut fm2 = ymap("name: consumer");
        let combined = merge_resolved(&mut fm2, "Only consumer.", &imports).unwrap();
        assert_eq!(combined, "Only consumer.");
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
