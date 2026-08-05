//! Field-specific merge policy for compile-time reusable imports.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde_yaml::{Mapping, Value};

use super::schema::apply_import_inputs;
use super::{ManifestFetcher, ResolvedImport, resolve_imports_with_repo_root};
use crate::compile::custom_tools::COMPONENT_PROVENANCE_KEYS;
use crate::compile::types::{ImportEntry, ParsedImportSpec, PermissionsRequired};

const CONSUMER_OWNED_FIELDS: &[&str] = &[
    "name",
    "description",
    "target",
    "engine",
    "workspace",
    "pool",
    "on",
    "permissions",
    "variable-groups",
    "parameters",
    "setup",
    "teardown",
    "execution-context",
    "supply-chain",
    "ado-aw-debug",
    "inlined-imports",
];

/// Resolve, substitute, and merge imports into a consumer mapping.
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

fn join_bodies(imported_body: &str, consumer_body: &str) -> String {
    let consumer_body = consumer_body.trim();
    match (imported_body.is_empty(), consumer_body.is_empty()) {
        (true, _) => consumer_body.to_string(),
        (false, true) => imported_body.to_string(),
        (false, false) => format!("{imported_body}\n\n{consumer_body}"),
    }
}

#[cfg(test)]
pub fn merge_resolved(
    consumer_fm: &mut Mapping,
    consumer_body: &str,
    resolved: &[ResolvedImport],
) -> Result<String> {
    let imported_body = merge_resolved_imported_body(consumer_fm, resolved)?;
    Ok(join_bodies(&imported_body, consumer_body))
}

/// Merge already-resolved imports and return their substituted prompt prefix.
pub fn merge_resolved_imported_body(
    consumer_fm: &mut Mapping,
    resolved: &[ResolvedImport],
) -> Result<String> {
    let mut state = MergeState::default();
    let mut body_parts = Vec::new();

    for import in resolved {
        let (mut front_matter, body) =
            apply_import_inputs(&import.front_matter, &import.body, &import.entry.with)
                .with_context(|| {
                    format!(
                        "failed to apply import inputs for `{}`",
                        import.provenance.source
                    )
                })?;
        stamp_component_provenance(&mut front_matter, import);
        if let Value::Mapping(mapping) = front_matter {
            state.merge_import(&mapping, &import.provenance.source)?;
        }
        let body = body.trim();
        if !body.is_empty() {
            body_parts.push(body.to_string());
        }
    }

    state.overlay_consumer(consumer_fm)?;
    dedupe_repos(&mut state.merged)?;
    state.merged.remove(Value::String("imports".to_string()));
    *consumer_fm = state.merged;
    Ok(body_parts.join("\n\n"))
}

#[derive(Default)]
struct MergeState {
    merged: Mapping,
    env_origins: HashMap<String, String>,
    mcp_origins: HashMap<String, String>,
    safe_output_origins: HashMap<String, String>,
}

impl MergeState {
    fn merge_import(&mut self, component: &Mapping, source: &str) -> Result<()> {
        for (key, value) in component {
            let Some(key) = key.as_str() else {
                continue;
            };
            match key {
                "imports" | "import-schema" => {}
                "tools" => merge_tools_import(&mut self.merged, value),
                "runtimes" => deep_fill_field(&mut self.merged, key, value),
                "mcp-servers" => {
                    merge_first_mapping(&mut self.merged, key, value, &mut self.mcp_origins)?
                }
                "env" => merge_unique_mapping(
                    &mut self.merged,
                    key,
                    value,
                    source,
                    &mut self.env_origins,
                )?,
                "network" => merge_import_network(&mut self.merged, value, source)?,
                "permissions-required" => merge_permissions_required(&mut self.merged, value)?,
                "safe-outputs" => merge_import_safe_outputs(
                    &mut self.merged,
                    value,
                    source,
                    &mut self.safe_output_origins,
                )?,
                "repos" | "steps" | "post-steps" => {
                    append_sequence(&mut self.merged, key, value)?;
                }
                unsupported => warn_ignored_field(source, unsupported),
            }
        }
        Ok(())
    }

    fn overlay_consumer(&mut self, consumer: &Mapping) -> Result<()> {
        for (key, value) in consumer {
            let Some(key) = key.as_str() else {
                continue;
            };
            match key {
                "imports" => {}
                "tools" => merge_tools_consumer(&mut self.merged, value),
                "runtimes" => deep_merge_field(&mut self.merged, key, value),
                "mcp-servers" => overlay_mapping_missing(&mut self.merged, key, value)?,
                "env" => overlay_mapping(&mut self.merged, key, value)?,
                "network" => merge_consumer_network(&mut self.merged, value)?,
                "permissions-required" => {
                    // Requirements can only be strengthened. The consumer may
                    // request additional capabilities but cannot clear an
                    // imported requirement.
                    merge_permissions_required(&mut self.merged, value)?;
                }
                "safe-outputs" => overlay_consumer_safe_outputs(&mut self.merged, value)?,
                "repos" => overlay_consumer_repos(&mut self.merged, value)?,
                "steps" => append_sequence(&mut self.merged, key, value)?,
                "post-steps" => prepend_sequence(&mut self.merged, key, value)?,
                _ => {
                    self.merged
                        .insert(Value::String(key.to_string()), value.clone());
                }
            }
        }
        Ok(())
    }
}

fn warn_ignored_field(source: &str, field: &str) {
    if CONSUMER_OWNED_FIELDS.contains(&field) {
        eprintln!(
            "Warning: imported component `{source}` sets consumer-owned field `{field}`; ignoring it"
        );
    } else {
        eprintln!(
            "Warning: imported component `{source}` sets unsupported field `{field}`; ignoring it"
        );
    }
}

fn deep_merge_field(target: &mut Mapping, key: &str, incoming: &Value) {
    let key = Value::String(key.to_string());
    match target.get_mut(&key) {
        Some(existing) => deep_merge_value(existing, incoming),
        None => {
            target.insert(key, incoming.clone());
        }
    }
}

fn deep_merge_value(existing: &mut Value, incoming: &Value) {
    match (existing, incoming) {
        (Value::Mapping(existing), Value::Mapping(incoming)) => {
            for (key, value) in incoming {
                match existing.get_mut(key) {
                    Some(current) => deep_merge_value(current, value),
                    None => {
                        existing.insert(key.clone(), value.clone());
                    }
                }
            }
        }
        (existing, incoming) => *existing = incoming.clone(),
    }
}

fn deep_fill_field(target: &mut Mapping, key: &str, incoming: &Value) {
    let key = Value::String(key.to_string());
    match target.get_mut(&key) {
        Some(existing) => deep_fill_value(existing, incoming),
        None => {
            target.insert(key, incoming.clone());
        }
    }
}

fn deep_fill_value(existing: &mut Value, incoming: &Value) {
    if let (Value::Mapping(existing), Value::Mapping(incoming)) = (existing, incoming) {
        for (key, value) in incoming {
            match existing.get_mut(key) {
                Some(current) => deep_fill_value(current, value),
                None => {
                    existing.insert(key.clone(), value.clone());
                }
            }
        }
    }
    // Earlier imports own any existing non-mapping field, including
    // sequences. Later imports only fill fields that remain absent.
}

fn merge_tools_import(target: &mut Mapping, incoming: &Value) {
    merge_tools_field(target, incoming, false);
}

fn merge_tools_consumer(target: &mut Mapping, incoming: &Value) {
    merge_tools_field(target, incoming, true);
}

fn merge_tools_field(target: &mut Mapping, incoming: &Value, consumer: bool) {
    let key = Value::String("tools".to_string());
    match target.get_mut(&key) {
        Some(existing) => merge_tools_value(existing, incoming, consumer),
        None => {
            target.insert(key, incoming.clone());
        }
    }
}

fn merge_tools_value(existing: &mut Value, incoming: &Value, consumer: bool) {
    match (existing, incoming) {
        (Value::Mapping(existing), Value::Mapping(incoming)) => {
            for (key, value) in incoming {
                match existing.get_mut(key) {
                    Some(current) => merge_tools_value(current, value, consumer),
                    None => {
                        existing.insert(key.clone(), value.clone());
                    }
                }
            }
        }
        (Value::Sequence(existing), Value::Sequence(incoming)) => {
            for value in incoming {
                if !existing.contains(value) {
                    existing.push(value.clone());
                }
            }
        }
        (existing, incoming) if consumer => *existing = incoming.clone(),
        // Earlier imports win for scalar/type-conflicting leaves.
        _ => {}
    }
}

fn merge_first_mapping(
    target: &mut Mapping,
    field: &str,
    incoming: &Value,
    origins: &mut HashMap<String, String>,
) -> Result<()> {
    let incoming = incoming
        .as_mapping()
        .with_context(|| format!("imported `{field}` must be a mapping"))?;
    let target_mapping = ensure_mapping_field(target, field)?;
    for (key, value) in incoming {
        let Some(name) = key.as_str() else {
            anyhow::bail!("imported `{field}` keys must be strings");
        };
        if origins.contains_key(name) {
            continue;
        }
        origins.insert(name.to_string(), String::new());
        target_mapping.insert(key.clone(), value.clone());
    }
    Ok(())
}

fn merge_unique_mapping(
    target: &mut Mapping,
    field: &str,
    incoming: &Value,
    source: &str,
    origins: &mut HashMap<String, String>,
) -> Result<()> {
    let incoming = incoming
        .as_mapping()
        .with_context(|| format!("imported `{field}` must be a mapping"))?;
    let target_value = target
        .entry(Value::String(field.to_string()))
        .or_insert_with(|| Value::Mapping(Mapping::new()));
    let target_mapping = target_value
        .as_mapping_mut()
        .with_context(|| format!("merged `{field}` must remain a mapping"))?;

    for (key, value) in incoming {
        let Some(name) = key.as_str() else {
            anyhow::bail!("imported `{field}` keys must be strings");
        };
        if let Some(previous) = origins.get(name) {
            anyhow::bail!(
                "import conflict: `{field}.{name}` is defined by both `{previous}` and `{source}`"
            );
        }
        origins.insert(name.to_string(), source.to_string());
        target_mapping.insert(key.clone(), value.clone());
    }
    Ok(())
}

fn overlay_mapping(target: &mut Mapping, field: &str, incoming: &Value) -> Result<()> {
    let incoming = incoming
        .as_mapping()
        .with_context(|| format!("consumer `{field}` must be a mapping"))?;
    let target_value = target
        .entry(Value::String(field.to_string()))
        .or_insert_with(|| Value::Mapping(Mapping::new()));
    let target_mapping = target_value
        .as_mapping_mut()
        .with_context(|| format!("merged `{field}` must remain a mapping"))?;
    for (key, value) in incoming {
        target_mapping.insert(key.clone(), value.clone());
    }
    Ok(())
}

fn overlay_mapping_missing(target: &mut Mapping, field: &str, incoming: &Value) -> Result<()> {
    let incoming = incoming
        .as_mapping()
        .with_context(|| format!("consumer `{field}` must be a mapping"))?;
    let target_mapping = ensure_mapping_field(target, field)?;
    for (key, value) in incoming {
        if !target_mapping.contains_key(key) {
            target_mapping.insert(key.clone(), value.clone());
        }
    }
    Ok(())
}

fn merge_import_network(target: &mut Mapping, incoming: &Value, source: &str) -> Result<()> {
    let incoming = incoming
        .as_mapping()
        .context("imported `network` must be a mapping")?;
    let network = ensure_mapping_field(target, "network")?;
    for (key, value) in incoming {
        match key.as_str() {
            Some("allowed") => union_string_sequence(network, "allowed", value)?,
            Some("blocked") if !value.as_sequence().is_none_or(Vec::is_empty) => eprintln!(
                "Warning: imported component `{source}` sets `network.blocked`; ignoring it \
                 because network deny policy is consumer-owned"
            ),
            Some("blocked") => {}
            Some(other) => eprintln!(
                "Warning: imported component `{source}` sets unsupported `network.{other}`; ignoring it"
            ),
            None => {}
        }
    }
    Ok(())
}

fn merge_consumer_network(target: &mut Mapping, incoming: &Value) -> Result<()> {
    let incoming = incoming
        .as_mapping()
        .context("consumer `network` must be a mapping")?;
    let network = ensure_mapping_field(target, "network")?;
    for (key, value) in incoming {
        if key.as_str() == Some("allowed") {
            union_string_sequence(network, "allowed", value)?;
        } else {
            network.insert(key.clone(), value.clone());
        }
    }
    Ok(())
}

fn union_string_sequence(target: &mut Mapping, field: &str, incoming: &Value) -> Result<()> {
    let incoming = incoming
        .as_sequence()
        .with_context(|| format!("`network.{field}` must be a sequence"))?;
    let target_value = target
        .entry(Value::String(field.to_string()))
        .or_insert_with(|| Value::Sequence(Vec::new()));
    let target_sequence = target_value
        .as_sequence_mut()
        .with_context(|| format!("merged `network.{field}` must remain a sequence"))?;
    for value in incoming {
        if !target_sequence.contains(value) {
            target_sequence.push(value.clone());
        }
    }
    Ok(())
}

fn merge_permissions_required(target: &mut Mapping, incoming: &Value) -> Result<()> {
    let mut requirements = target
        .get(Value::String("permissions-required".to_string()))
        .map(|value| serde_yaml::from_value::<PermissionsRequired>(value.clone()))
        .transpose()
        .context("merged `permissions-required` is invalid")?
        .unwrap_or_default();
    let incoming: PermissionsRequired = serde_yaml::from_value(incoming.clone())
        .context("`permissions-required` must contain boolean `read` / `write` fields")?;
    requirements.union(incoming);
    target.insert(
        Value::String("permissions-required".to_string()),
        serde_yaml::to_value(requirements).context("failed to serialize permissions-required")?,
    );
    Ok(())
}

fn merge_import_safe_outputs(
    target: &mut Mapping,
    incoming: &Value,
    source: &str,
    origins: &mut HashMap<String, String>,
) -> Result<()> {
    let incoming = incoming
        .as_mapping()
        .context("imported `safe-outputs` must be a mapping")?;
    let safe_outputs = ensure_mapping_field(target, "safe-outputs")?;
    for (key, value) in incoming {
        let Some(name) = key.as_str() else {
            anyhow::bail!("imported `safe-outputs` keys must be strings");
        };
        match name {
            "scripts" => {
                eprintln!(
                    "Warning: imported component `{source}` uses removed \
                     `safe-outputs.scripts`; ignoring it"
                );
            }
            "jobs" => {
                merge_import_custom_jobs(safe_outputs, value, source, origins)?;
            }
            _ => {
                let origin_key = format!("safe-outputs.{name}");
                if let Some(previous) = origins.get(&origin_key) {
                    anyhow::bail!(
                        "import conflict: `{origin_key}` is defined by both `{previous}` and `{source}`"
                    );
                }
                origins.insert(origin_key, source.to_string());
                safe_outputs.insert(key.clone(), value.clone());
            }
        }
    }
    Ok(())
}

fn merge_import_custom_jobs(
    safe_outputs: &mut Mapping,
    incoming: &Value,
    source: &str,
    origins: &mut HashMap<String, String>,
) -> Result<()> {
    let incoming = incoming
        .as_mapping()
        .context("imported `safe-outputs.jobs` must be a mapping")?;
    let jobs_value = safe_outputs
        .entry(Value::String("jobs".to_string()))
        .or_insert_with(|| Value::Mapping(Mapping::new()));
    let jobs = jobs_value
        .as_mapping_mut()
        .context("merged `safe-outputs.jobs` must remain a mapping")?;
    for (key, value) in incoming {
        let Some(name) = key.as_str() else {
            anyhow::bail!("imported custom safe-output job names must be strings");
        };
        let origin_key = format!("safe-outputs.jobs.{name}");
        if let Some(previous) = origins.get(&origin_key) {
            anyhow::bail!(
                "import conflict: `{origin_key}` is defined by both `{previous}` and `{source}`"
            );
        }
        origins.insert(origin_key, source.to_string());
        jobs.insert(key.clone(), value.clone());
    }
    Ok(())
}

fn overlay_consumer_safe_outputs(target: &mut Mapping, incoming: &Value) -> Result<()> {
    let incoming = incoming
        .as_mapping()
        .context("consumer `safe-outputs` must be a mapping")?;
    let safe_outputs = ensure_mapping_field(target, "safe-outputs")?;
    for (key, value) in incoming {
        if key.as_str() == Some("jobs") {
            overlay_consumer_custom_jobs(safe_outputs, value)?;
        } else {
            // Built-in safe-output configuration is consumer-owned.
            safe_outputs.insert(key.clone(), value.clone());
        }
    }
    Ok(())
}

fn overlay_consumer_custom_jobs(safe_outputs: &mut Mapping, incoming: &Value) -> Result<()> {
    let incoming = incoming
        .as_mapping()
        .context("consumer `safe-outputs.jobs` must be a mapping")?;
    let jobs_value = safe_outputs
        .entry(Value::String("jobs".to_string()))
        .or_insert_with(|| Value::Mapping(Mapping::new()));
    let jobs = jobs_value
        .as_mapping_mut()
        .context("merged `safe-outputs.jobs` must remain a mapping")?;
    for (key, incoming_job) in incoming {
        let Some(name) = key.as_str() else {
            anyhow::bail!("custom safe-output job names must be strings");
        };
        if jobs.contains_key(key) {
            anyhow::bail!(
                "import conflict: custom safe-output job `{name}` is already defined by an \
                 imported component; configure approval/staged policy at \
                 `safe-outputs.{name}` instead of redeclaring the job"
            );
        }
        jobs.insert(key.clone(), incoming_job.clone());
    }
    Ok(())
}

fn append_sequence(target: &mut Mapping, field: &str, incoming: &Value) -> Result<()> {
    let incoming = incoming
        .as_sequence()
        .with_context(|| format!("`{field}` must be a sequence"))?;
    let target_value = target
        .entry(Value::String(field.to_string()))
        .or_insert_with(|| Value::Sequence(Vec::new()));
    let target_sequence = target_value
        .as_sequence_mut()
        .with_context(|| format!("merged `{field}` must remain a sequence"))?;
    target_sequence.extend(incoming.iter().cloned());
    Ok(())
}

fn prepend_sequence(target: &mut Mapping, field: &str, incoming: &Value) -> Result<()> {
    let incoming = incoming
        .as_sequence()
        .with_context(|| format!("`{field}` must be a sequence"))?;
    let imported = target
        .remove(Value::String(field.to_string()))
        .and_then(|value| value.as_sequence().cloned())
        .unwrap_or_default();
    let mut combined = incoming.clone();
    combined.extend(imported);
    target.insert(Value::String(field.to_string()), Value::Sequence(combined));
    Ok(())
}

fn overlay_consumer_repos(target: &mut Mapping, incoming: &Value) -> Result<()> {
    let consumer = incoming
        .as_sequence()
        .context("consumer `repos` must be a sequence")?;
    let imported = target
        .remove(Value::String("repos".to_string()))
        .and_then(|value| value.as_sequence().cloned())
        .unwrap_or_default();
    let mut merged = Vec::new();
    let mut seen = HashMap::<String, ()>::new();
    for repo in consumer.iter().chain(imported.iter()) {
        let key = repo_identity(repo);
        if seen.insert(key, ()).is_none() {
            merged.push(repo.clone());
        }
    }
    target.insert(Value::String("repos".to_string()), Value::Sequence(merged));
    Ok(())
}

fn dedupe_repos(target: &mut Mapping) -> Result<()> {
    let Some(repos) = target.remove(Value::String("repos".to_string())) else {
        return Ok(());
    };
    let repos = repos
        .as_sequence()
        .context("merged `repos` must be a sequence")?;
    let mut merged = Vec::new();
    let mut seen = HashMap::<String, ()>::new();
    for repo in repos {
        let key = repo_identity(repo);
        if seen.insert(key, ()).is_none() {
            merged.push(repo.clone());
        }
    }
    target.insert(Value::String("repos".to_string()), Value::Sequence(merged));
    Ok(())
}

fn repo_identity(value: &Value) -> String {
    match value {
        Value::String(value) => value
            .split_once('=')
            .map(|(alias, _)| format!("alias:{alias}"))
            .unwrap_or_else(|| format!("name:{value}")),
        Value::Mapping(mapping) => mapping
            .get(Value::String("alias".to_string()))
            .and_then(Value::as_str)
            .map(|value| format!("alias:{value}"))
            .or_else(|| {
                mapping
                    .get(Value::String("name".to_string()))
                    .and_then(Value::as_str)
                    .map(|value| format!("name:{value}"))
            })
            .unwrap_or_else(|| yaml_identity(value)),
        _ => yaml_identity(value),
    }
}

fn yaml_identity(value: &Value) -> String {
    serde_yaml::to_string(value).unwrap_or_else(|_| format!("{value:?}"))
}

fn ensure_mapping_field<'a>(target: &'a mut Mapping, field: &str) -> Result<&'a mut Mapping> {
    target
        .entry(Value::String(field.to_string()))
        .or_insert_with(|| Value::Mapping(Mapping::new()))
        .as_mapping_mut()
        .with_context(|| format!("merged `{field}` must remain a mapping"))
}

fn stamp_component_provenance(component_fm: &mut Value, import: &ResolvedImport) {
    let Value::Mapping(front_matter) = component_fm else {
        return;
    };
    let Some(Value::Mapping(safe_outputs)) = front_matter.get_mut("safe-outputs") else {
        return;
    };
    let Some(Value::Mapping(jobs)) = safe_outputs.get_mut("jobs") else {
        return;
    };

    let is_remote = matches!(import.spec, ParsedImportSpec::Remote { .. });
    for job in jobs.values_mut() {
        let Value::Mapping(job) = job else {
            continue;
        };
        for key in COMPONENT_PROVENANCE_KEYS {
            job.remove(Value::String(key.to_string()));
        }
        if is_remote && let Some(sha) = &import.provenance.sha {
            job.insert(
                Value::String("component-source".to_string()),
                Value::String(import.provenance.source.clone()),
            );
            if let Some(requested_ref) = &import.provenance.requested_ref {
                job.insert(
                    Value::String("component-ref".to_string()),
                    Value::String(requested_ref.clone()),
                );
            }
            job.insert(
                Value::String("component-sha".to_string()),
                Value::String(sha.clone()),
            );
            job.insert(
                Value::String("manifest-digest".to_string()),
                Value::String(import.provenance.manifest_digest.clone()),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::types::{ImportSource, ParsedImportSpec};

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    fn ymap(yaml: &str) -> Mapping {
        serde_yaml::from_str::<Value>(yaml)
            .unwrap()
            .as_mapping()
            .unwrap()
            .clone()
    }

    fn local(front_matter: &str, body: &str) -> ResolvedImport {
        ResolvedImport {
            entry: ImportEntry {
                uses: "./component.md".to_string(),
                with: Default::default(),
                repository: None,
                source: None,
            },
            spec: ParsedImportSpec::Local {
                path: "./component.md".to_string(),
                section: None,
                optional: false,
            },
            front_matter: serde_yaml::from_str(front_matter).unwrap(),
            body: body.to_string(),
            provenance: super::super::ImportProvenance {
                source: "component.md".to_string(),
                requested_ref: None,
                sha: None,
                manifest_digest: "digest".to_string(),
            },
        }
    }

    fn remote(front_matter: &str) -> ResolvedImport {
        ResolvedImport {
            entry: ImportEntry {
                uses: "component.md@main".to_string(),
                with: Default::default(),
                repository: Some("owner/repo".to_string()),
                source: Some(ImportSource::GitHub {
                    host: crate::secure::HostName::parse("github.com").unwrap(),
                }),
            },
            spec: ParsedImportSpec::Remote {
                source: ImportSource::GitHub {
                    host: crate::secure::HostName::parse("github.com").unwrap(),
                },
                project: Some("owner".to_string()),
                repository: "repo".to_string(),
                path: "component.md".to_string(),
                requested_ref: "main".to_string(),
                section: None,
                optional: false,
            },
            front_matter: serde_yaml::from_str(front_matter).unwrap(),
            body: String::new(),
            provenance: super::super::ImportProvenance {
                source: "github:github.com/owner/repo/component.md".to_string(),
                requested_ref: Some("main".to_string()),
                sha: Some(SHA.to_string()),
                manifest_digest: "digest".to_string(),
            },
        }
    }

    #[test]
    fn consumer_owned_import_fields_are_ignored() {
        let mut consumer = ymap("name: consumer\ndescription: root\ntarget: standalone");
        merge_resolved(
            &mut consumer,
            "",
            &[local(
                "name: component\ndescription: ignored\ntarget: 1es\nengine: claude",
                "",
            )],
        )
        .unwrap();
        assert_eq!(consumer["name"], "consumer");
        assert_eq!(consumer["target"], "standalone");
        assert!(!consumer.contains_key("engine"));
    }

    #[test]
    fn tools_union_allow_arrays_and_consumer_scalars_win() {
        let mut consumer = ymap(
            "tools:\n  edit: false\n  azure-devops:\n    allowed: [b, consumer]\n    org: consumer",
        );
        merge_resolved(
            &mut consumer,
            "",
            &[
                local(
                    r#"tools:
  edit: true
  azure-devops:
    allowed: [a, b]
    toolsets: [repos]
    org: first"#,
                    "",
                ),
                local(
                    r#"tools:
  edit: true
  azure-devops:
    allowed: [b, c]
    toolsets: [wit]
    org: second"#,
                    "",
                ),
            ],
        )
        .unwrap();
        assert_eq!(consumer["tools"]["edit"], false);
        assert_eq!(consumer["tools"]["azure-devops"]["org"], "consumer");
        assert_eq!(
            consumer["tools"]["azure-devops"]["allowed"],
            serde_yaml::from_str::<Value>("[a, b, c, consumer]").unwrap()
        );
        assert_eq!(
            consumer["tools"]["azure-devops"]["toolsets"],
            serde_yaml::from_str::<Value>("[repos, wit]").unwrap()
        );
    }

    #[test]
    fn runtimes_consumer_overrides_and_earlier_imports_fill_remaining_fields() {
        let mut consumer = ymap("runtimes:\n  python:\n    version: '3.12'");
        merge_resolved(
            &mut consumer,
            "",
            &[
                local(
                    "runtimes:\n  python:\n    version: '3.10'\n    packages: [first]",
                    "",
                ),
                local(
                    r#"runtimes:
  python:
    version: '3.11'
    packages: [second]
    architecture: x64
  node:
    version: '22'"#,
                    "",
                ),
            ],
        )
        .unwrap();
        assert_eq!(consumer["runtimes"]["python"]["version"], "3.12");
        assert_eq!(
            consumer["runtimes"]["python"]["packages"],
            serde_yaml::from_str::<Value>("[first]").unwrap()
        );
        assert_eq!(consumer["runtimes"]["python"]["architecture"], "x64");
        assert_eq!(consumer["runtimes"]["node"]["version"], "22");
    }

    #[test]
    fn mcp_servers_first_import_wins_and_import_overrides_consumer() {
        let mut consumer = ymap(
            r#"mcp-servers:
  shared:
    url: https://consumer.example
  consumer-only:
    url: https://consumer-only.example"#,
        );
        merge_resolved(
            &mut consumer,
            "",
            &[
                local(
                    r#"mcp-servers:
  shared:
    url: https://first.example
  first-only:
    url: https://first-only.example"#,
                    "",
                ),
                local(
                    r#"mcp-servers:
  shared:
    url: https://second.example
  second-only:
    url: https://second-only.example"#,
                    "",
                ),
            ],
        )
        .unwrap();
        assert_eq!(
            consumer["mcp-servers"]["shared"]["url"],
            "https://first.example"
        );
        assert_eq!(
            consumer["mcp-servers"]["consumer-only"]["url"],
            "https://consumer-only.example"
        );
        assert_eq!(
            consumer["mcp-servers"]["first-only"]["url"],
            "https://first-only.example"
        );
        assert_eq!(
            consumer["mcp-servers"]["second-only"]["url"],
            "https://second-only.example"
        );
    }

    #[test]
    fn env_duplicates_between_imports_fail_but_consumer_overrides() {
        let imports = [local("env:\n  A: one", ""), local("env:\n  A: two", "")];
        let error = merge_resolved(&mut ymap("name: c"), "", &imports).unwrap_err();
        assert!(error.to_string().contains("env.A"));

        let mut consumer = ymap("env:\n  A: consumer");
        merge_resolved(&mut consumer, "", &[local("env:\n  A: imported", "")]).unwrap();
        assert_eq!(consumer["env"]["A"], "consumer");
    }

    #[test]
    fn network_permissions_and_sequence_orders_follow_contract() {
        let mut consumer = ymap(
            "network:\n  allowed: [consumer.example]\n  blocked: [deny.example]\n\
             permissions-required:\n  read: false\n\
             repos: [shared/repo, consumer/repo]\n\
             steps:\n  - bash: consumer-step\n\
             post-steps:\n  - bash: consumer-post",
        );
        merge_resolved(
            &mut consumer,
            "",
            &[local(
                "network:\n  allowed: [import.example]\n  blocked: [ignored.example]\n\
                 permissions-required:\n  read: true\n  write: true\n\
                 repos: [shared/repo, imported/repo]\n\
                 steps:\n  - bash: import-step\n\
                 post-steps:\n  - bash: import-post",
                "",
            )],
        )
        .unwrap();
        assert_eq!(
            consumer["network"]["allowed"].as_sequence().unwrap().len(),
            2
        );
        assert_eq!(consumer["permissions-required"]["read"], true);
        assert_eq!(consumer["permissions-required"]["write"], true);
        assert_eq!(consumer["repos"][0], "shared/repo");
        assert_eq!(consumer["repos"][1], "consumer/repo");
        assert_eq!(consumer["repos"][2], "imported/repo");
        assert_eq!(consumer["steps"][0]["bash"], "import-step");
        assert_eq!(consumer["steps"][1]["bash"], "consumer-step");
        assert_eq!(consumer["post-steps"][0]["bash"], "consumer-post");
        assert_eq!(consumer["post-steps"][1]["bash"], "import-post");
    }

    #[test]
    fn imported_repos_are_deduplicated_without_consumer_repos() {
        let mut consumer = ymap("name: consumer");
        merge_resolved(
            &mut consumer,
            "",
            &[
                local("repos: [shared/repo, other/repo]", ""),
                local("repos: [shared/repo]", ""),
            ],
        )
        .unwrap();
        assert_eq!(consumer["repos"].as_sequence().unwrap().len(), 2);
    }

    #[test]
    fn safe_output_duplicates_fail_and_consumer_builtins_override() {
        let duplicate = [
            local("safe-outputs:\n  create-github-issue:\n    max: 1", ""),
            local("safe-outputs:\n  create-github-issue:\n    max: 2", ""),
        ];
        assert!(
            merge_resolved(&mut ymap("name: c"), "", &duplicate)
                .unwrap_err()
                .to_string()
                .contains("safe-outputs.create-github-issue")
        );

        let mut consumer = ymap("safe-outputs:\n  create-github-issue:\n    max: 9");
        merge_resolved(
            &mut consumer,
            "",
            &[local("safe-outputs:\n  create-github-issue:\n    max: 1", "")],
        )
        .unwrap();
        assert_eq!(consumer["safe-outputs"]["create-github-issue"]["max"], 9);
    }

    #[test]
    fn custom_job_names_are_unique_and_policy_stays_top_level() {
        let import = remote(
            r#"safe-outputs:
  jobs:
    notify:
      steps:
        - bash: echo hi
      component-ref: attacker-controlled
      max: 2"#,
        );
        let mut consumer = ymap("safe-outputs:\n  notify:\n    require-approval: true\n    max: 4");
        merge_resolved(&mut consumer, "", std::slice::from_ref(&import)).unwrap();
        let job = &consumer["safe-outputs"]["jobs"]["notify"];
        assert_eq!(job["max"], 2);
        assert_eq!(job["component-ref"], "main");
        assert_eq!(job["component-sha"], SHA);
        assert_eq!(job["manifest-digest"], "digest");
        assert!(job.get("component-repo-type").is_none());
        assert!(job.get("component-endpoint").is_none());
        assert_eq!(consumer["safe-outputs"]["notify"]["require-approval"], true);
        assert_eq!(consumer["safe-outputs"]["notify"]["max"], 4);

        let mut consumer = ymap("safe-outputs:\n  jobs:\n    notify:\n      steps: []");
        let error = merge_resolved(&mut consumer, "", std::slice::from_ref(&import)).unwrap_err();
        assert!(error.to_string().contains("already defined"), "{error}");

        let duplicate_import =
            remote("safe-outputs:\n  jobs:\n    notify:\n      steps:\n        - bash: duplicate");
        let error = merge_resolved(&mut ymap("name: consumer"), "", &[import, duplicate_import])
            .unwrap_err();
        assert!(
            error.to_string().contains("safe-outputs.jobs.notify"),
            "{error}"
        );
    }

    #[test]
    fn imported_bodies_precede_consumer_body() {
        let body = merge_resolved(
            &mut ymap("name: c"),
            "Consumer.",
            &[local("{}", "First."), local("{}", "Second.")],
        )
        .unwrap();
        assert_eq!(body, "First.\n\nSecond.\n\nConsumer.");
    }
}
