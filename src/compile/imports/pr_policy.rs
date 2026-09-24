//! PR migration identities used before import precedence is resolved.

use std::collections::HashSet;

use anyhow::{Context, Result, ensure};
use serde_yaml::{Mapping, Value};

use crate::compile::pr_migration::{LEGACY_PR_CONFIG, PR_OPERATIONS, PR_TOOL_RENAMES};

pub(super) fn custom_job_names<'a>(
    manifests: impl IntoIterator<Item = &'a Mapping>,
) -> Result<HashSet<String>> {
    let mut names = HashSet::new();
    for manifest in manifests {
        if let Some(jobs) = manifest
            .get("safe-outputs")
            .and_then(|outputs| outputs.get("jobs"))
        {
            for name in jobs
                .as_mapping()
                .context("safe-outputs.jobs must be a mapping")?
                .keys()
            {
                names.insert(
                    name.as_str()
                        .context("custom safe-output job names must be strings")?
                        .to_string(),
                );
            }
        }
    }
    Ok(names)
}

/// Canonicalize identities, not policies: overridden defaults need not be valid.
pub(crate) fn rename_declarations(
    manifest: &mut Mapping,
    custom_jobs: &HashSet<String>,
) -> Result<bool> {
    let Some(Value::Mapping(outputs)) = manifest.get("safe-outputs") else {
        return Ok(false);
    };
    let mut renamed = outputs.clone();
    for (old, new) in PR_TOOL_RENAMES {
        if custom_jobs.contains(*old) {
            continue;
        }
        if let Some(value) = renamed.remove(*old) {
            ensure!(
                !renamed.contains_key(*new),
                "manual migration required: both {old} and {new} are configured"
            );
            renamed.insert(Value::String((*new).to_string()), value);
        }
    }
    if let Some(Value::Mapping(groups)) = renamed.get_mut("budget-groups") {
        for group in groups.values_mut() {
            if let Some(Value::Sequence(tools)) = group.get_mut("tools") {
                for tool in tools {
                    if let Some(name) = tool.as_str()
                        && !custom_jobs.contains(name)
                        && let Some((_, new)) = PR_TOOL_RENAMES.iter().find(|(old, _)| *old == name)
                    {
                        *tool = Value::String((*new).to_string());
                    }
                }
            }
        }
    }
    let changed = *outputs != renamed;
    if changed {
        manifest.insert(
            Value::String("safe-outputs".to_string()),
            Value::Mapping(renamed),
        );
    }
    Ok(changed)
}

/// Apply a pure built-in transformation without touching custom-job policies.
pub(crate) fn transform_builtins(
    manifest: &mut Mapping,
    custom_jobs: &HashSet<String>,
    transform: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>) -> Result<bool>,
) -> Result<bool> {
    let Some(raw) = manifest.get("safe-outputs") else {
        return Ok(false);
    };
    let value = serde_json::to_value(raw)?;
    let Some(mut outputs) = value.as_object().cloned() else {
        return Ok(false);
    };
    let custom = custom_jobs
        .iter()
        .filter_map(|name| outputs.remove(name).map(|value| (name.clone(), value)))
        .collect::<Vec<_>>();
    let changed = transform(&mut outputs)?;
    if changed {
        outputs.extend(custom);
        manifest.insert(
            Value::String("safe-outputs".to_string()),
            serde_yaml::to_value(outputs)?,
        );
    }
    Ok(changed)
}

pub(crate) fn local_custom_job_names(manifest: &Mapping) -> Result<HashSet<String>> {
    custom_job_names(std::iter::once(manifest))
}

/// A migrated family is identified by its children and shared budget, never by
/// reconstructing configuration from the retained legacy constraints.
pub(super) fn legacy_family(
    outputs: &Mapping,
    custom_jobs: &HashSet<String>,
) -> Result<Option<HashSet<String>>> {
    let raw = outputs.contains_key("update-pr") && !custom_jobs.contains("update-pr");
    let mut children = HashSet::new();
    let mut original = None;
    for (tool, config) in outputs {
        let Some(tool) = tool.as_str() else {
            continue;
        };
        if custom_jobs.contains(tool) {
            continue;
        }
        let Some(metadata) = config.get(LEGACY_PR_CONFIG) else {
            continue;
        };
        ensure!(
            PR_OPERATIONS.iter().any(|(_, name)| *name == tool) && metadata.is_mapping(),
            "ambiguous legacy update-pr family: {tool}.{LEGACY_PR_CONFIG} must be an object on a focused PR tool"
        );
        if let Some(previous) = original {
            ensure!(
                previous == metadata,
                "ambiguous legacy update-pr family: children retain different legacy policies"
            );
        } else {
            original = Some(metadata);
        }
        children.insert(tool.to_string());
    }
    ensure!(
        !raw || children.is_empty(),
        "manual migration required: update-pr and migrated legacy update-pr children coexist"
    );
    if raw {
        return Ok(Some(HashSet::from(["update-pr".to_string()])));
    }
    if children.is_empty() {
        return Ok(None);
    }
    let tools = outputs
        .get("budget-groups")
        .and_then(|groups| groups.get("update-pr"))
        .and_then(|group| group.get("tools"))
        .and_then(Value::as_sequence)
        .context("ambiguous legacy update-pr family: missing update-pr budget group")?;
    let members = tools
        .iter()
        .map(|tool| {
            tool.as_str()
                .context("legacy update-pr budget members must be tool names")
        })
        .collect::<Result<HashSet<_>>>()?;
    ensure!(
        members.len() == tools.len()
            && members.len() == children.len()
            && children
                .iter()
                .all(|child| members.contains(child.as_str())),
        "ambiguous legacy update-pr family: budget members must exactly match its migrated children"
    );
    Ok(Some(children))
}

pub(super) fn replace_legacy_family(
    existing: &mut Mapping,
    incoming: &Mapping,
    custom_jobs: &HashSet<String>,
) -> Result<()> {
    if legacy_family(incoming, custom_jobs)?.is_some()
        && let Some(children) = legacy_family(existing, custom_jobs)?
    {
        let migrated = !children.contains("update-pr");
        for child in children {
            existing.remove(child);
        }
        if migrated && let Some(Value::Mapping(groups)) = existing.get_mut("budget-groups") {
            groups.remove("update-pr");
        }
    }
    Ok(())
}
