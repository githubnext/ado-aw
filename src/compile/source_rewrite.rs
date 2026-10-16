//! Apply codemod results to commented YAML without discarding source trivia.

use std::collections::BTreeMap;
use std::str::FromStr;

use anyhow::{Context, Result, ensure};
use serde_yaml::{Mapping, Value};
use yaml_edit::{Document, SyntaxKind, YamlFile, YamlNode};

use super::common::split_markdown_front_matter;

pub(crate) fn preserve_comments(original: &str, desired: &Mapping) -> Result<Option<String>> {
    let parts = split_markdown_front_matter(original, true)?;
    let yaml = parts.yaml_raw.context("Source has no YAML front matter")?;
    if !yaml.contains('#') {
        return Ok(None);
    }
    let comments = comment_counts(&yaml);
    if comments.is_empty() {
        return Ok(None);
    }
    let file = YamlFile::from_str(&yaml)
        .context("Cannot safely edit commented YAML; source was not rewritten")?;
    let documents: Vec<_> = file.documents().collect();
    ensure!(
        documents.len() == 1,
        "Comment-preserving rewrite requires one YAML document"
    );
    let tree = documents[0]
        .as_mapping()
        .context("Front matter must be a mapping")?;
    let before: Mapping = serde_yaml::from_str(&yaml)?;
    reconcile_mapping(&tree, &before, desired)?;
    let mut rendered = file.to_string();
    let actual: Mapping = serde_yaml::from_str(&rendered)
        .context("Comment-preserving rewrite produced invalid YAML; source was not rewritten")?;
    ensure!(
        actual == *desired,
        "Comment-preserving rewrite did not reproduce the codemod policy; source was not rewritten"
    );
    let remaining = comment_counts(&rendered);
    ensure!(
        comments
            .iter()
            .all(|(comment, count)| remaining.get(comment).copied().unwrap_or(0) >= *count),
        "Codemod would remove source comments; migrate the affected declaration manually before compiling"
    );
    if yaml.contains("\r\n") && !yaml.replace("\r\n", "").contains('\n') {
        rendered = rendered.replace("\r\n", "\n").replace('\n', "\r\n");
    }
    if !rendered.ends_with('\n') {
        rendered.push('\n');
    }
    Ok(Some(format!(
        "{}---{}---{}",
        parts.leading_whitespace, rendered, parts.body_raw
    )))
}

fn comment_counts(source: &str) -> BTreeMap<String, usize> {
    let mut comments = BTreeMap::new();
    for (kind, text) in yaml_edit::lex(source) {
        if kind == SyntaxKind::COMMENT {
            *comments.entry(text.to_string()).or_default() += 1;
        }
    }
    comments
}

fn node(value: &Value) -> Result<YamlNode> {
    let wrapped = Mapping::from_iter([(Value::String("value".into()), value.clone())]);
    let serialized = serde_yaml::to_string(&wrapped)?;
    let doc = Document::from_str(&serialized)?;
    doc.as_mapping()
        .and_then(|mapping| mapping.get("value"))
        .context("Could not materialize a codemod value")
}

fn renamed_key(key: &str, desired: &Mapping, before: &Mapping) -> Option<&'static str> {
    let renamed = super::pr_migration::PR_TOOL_RENAMES
        .iter()
        .find_map(|(old, new)| (*old == key).then_some(*new))
        .or(match key {
            "target-repo" | "repository" => Some("default-repository"),
            "squash-merge" => Some("merge-strategy"),
            "footer" => Some("include-stats"),
            _ => None,
        })?;
    (!before.contains_key(renamed) && desired.contains_key(renamed)).then_some(renamed)
}

fn reconcile_mapping(tree: &yaml_edit::Mapping, before: &Mapping, desired: &Mapping) -> Result<()> {
    let mut current = before.clone();
    for key in before.keys().filter(|key| !desired.contains_key(*key)) {
        let name = key
            .as_str()
            .context("Cannot safely rewrite a complex YAML key")?;
        if name == "update-pr"
            && let Some((child_key, child_value)) = desired.iter().find(|(_, value)| {
                let original = before.get(key);
                let legacy = value.get(super::pr_migration::LEGACY_PR_CONFIG);
                legacy == original
                    || (matches!(original, Some(Value::Null | Value::Bool(true)))
                        && legacy
                            .and_then(Value::as_mapping)
                            .is_some_and(Mapping::is_empty))
            })
        {
            let child = child_key
                .as_str()
                .context("Migrated PR tool name must be a string")?;
            let replacement = node(child_value)?;
            if before.get(key).is_some_and(Value::is_mapping) {
                let saved = tree
                    .get(name)
                    .context("Legacy PR configuration is unavailable")?;
                replacement
                    .as_mapping()
                    .context("Migrated PR policy must be a mapping")?
                    .set(super::pr_migration::LEGACY_PR_CONFIG, saved);
            }
            tree.rename_key(name, child);
            tree.set(child, replacement);
            current.remove(key);
            current.insert(child_key.clone(), child_value.clone());
            continue;
        }
        if let Some(new) = renamed_key(name, desired, before) {
            tree.rename_key(name, new);
            if let Some(value) = current.remove(key) {
                current.insert(Value::String(new.into()), value);
            }
        } else {
            tree.remove(name);
            current.remove(key);
        }
    }
    for (key, value) in desired {
        if current.get(key) == Some(value) {
            continue;
        }
        let name = key
            .as_str()
            .context("Cannot safely rewrite a complex YAML key")?;
        match (current.get(key), value) {
            (Some(Value::Mapping(old)), Value::Mapping(new)) => {
                let nested = tree
                    .get_mapping(name)
                    .context("YAML mapping structure changed during rewrite")?;
                reconcile_mapping(&nested, old, new)?;
            }
            (Some(Value::Sequence(old)), Value::Sequence(new)) if old.len() == new.len() => {
                let sequence = tree
                    .get_sequence(name)
                    .context("YAML sequence structure changed during rewrite")?;
                for (index, (old, new)) in old.iter().zip(new).enumerate() {
                    if old != new {
                        if let (Value::Mapping(old), Value::Mapping(new)) = (old, new) {
                            let nested = sequence
                                .get(index)
                                .and_then(|node| node.as_mapping().cloned())
                                .context("YAML sequence mapping is unavailable")?;
                            reconcile_mapping(&nested, old, new)?;
                        } else {
                            sequence.set(index, node(new)?);
                        }
                    }
                }
            }
            _ => {
                tree.set(name, node(value)?);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_comments_quotes_and_body_during_pr_migration() {
        let original = "---\n# Workflow explanation\nname: 'comments'\ndescription: test\nsafe-outputs:\n  # Review configuration\n  add-pr-comment:\n    target-repo: self # Repository explanation\n    max: 2 # Keep budget\n---\nBody # unchanged\n";
        let parsed = super::super::parse_markdown_detailed(original).unwrap();
        let rewritten = preserve_comments(original, &parsed.front_matter_mapping)
            .unwrap()
            .unwrap();
        for text in [
            "# Workflow explanation",
            "# Review configuration",
            "# Repository explanation",
            "# Keep budget",
            "name: 'comments'",
            "\nBody # unchanged\n",
        ] {
            assert!(rewritten.contains(text), "{text}: {rewritten}");
        }
        assert!(rewritten.contains("add-pull-request-comment:"));
        assert!(rewritten.contains("default-repository: self"));
    }

    #[test]
    fn refuses_to_discard_comments_inside_a_removed_value() {
        let original = "---\nname: test\nremoved:\n  key: value # retain this\n---\nBody\n";
        let mut desired: Mapping =
            serde_yaml::from_str("\nname: test\nremoved:\n  key: value\n").unwrap();
        desired.remove("removed");
        assert!(preserve_comments(original, &desired).is_err());
    }

    #[test]
    fn preserves_comment_text_and_line_endings_when_initial_completion_changes() {
        let original = "---\r\nname: legacy\r\ndescription: '#not a comment'\r\nsafe-outputs:\r\n  create-pull-request:\r\n    draft: true # keep draft\r\n    auto-complete: true # old ineffective setting\r\n    squash-merge: false # no squash\r\n---\r\nPrompt untouched.\r\n";
        let parsed =
            super::super::common::parse_markdown_detailed_for_source(original, Some("0.53.0"))
                .unwrap();
        let rewritten = preserve_comments(original, &parsed.front_matter_mapping)
            .unwrap()
            .unwrap();
        assert!(!rewritten.replace("\r\n", "").contains('\n'));
        for comment in ["# keep draft", "# old ineffective setting", "# no squash"] {
            assert!(rewritten.contains(comment), "{rewritten}");
        }
        assert!(rewritten.contains("merge-strategy: noFastForward"));
        assert!(rewritten.contains("auto-complete: false"));
        assert!(rewritten.ends_with("---\r\nPrompt untouched.\r\n"));
    }

    #[test]
    fn split_legacy_pr_policy_retains_comments_in_preserved_legacy_constraints() {
        let original = "---\nname: legacy\ndescription: test\nsafe-outputs:\n  # Bounded legacy authority\n  update-pr: # review settings\n    allowed-operations: [add-labels, update-description] # only these\n    max: 2 # shared limit\n---\nOriginal prompt\n";
        let parsed = super::super::parse_markdown_detailed(original).unwrap();
        let rewritten = preserve_comments(original, &parsed.front_matter_mapping)
            .unwrap()
            .unwrap();
        for comment in [
            "# Bounded legacy authority",
            "# review settings",
            "# only these",
            "# shared limit",
        ] {
            assert!(rewritten.contains(comment), "{rewritten}");
        }
        let reparsed = super::super::parse_markdown_detailed(&rewritten).unwrap();
        assert_eq!(reparsed.front_matter_mapping, parsed.front_matter_mapping);
        assert!(!reparsed.codemods.changed());
    }

    #[test]
    fn bare_legacy_declaration_retains_its_comment_without_granting_votes() {
        let original = "---\nname: bare\ndescription: test\nsafe-outputs:\n  update-pr: # automatic maintenance\n---\nBody\n";
        let parsed = super::super::parse_markdown_detailed(original).unwrap();
        let rewritten = preserve_comments(original, &parsed.front_matter_mapping)
            .unwrap()
            .unwrap();
        assert!(rewritten.contains("# automatic maintenance"));
        assert!(
            !parsed.front_matter_mapping["safe-outputs"]
                .as_mapping()
                .unwrap()
                .contains_key("submit-pull-request-review")
        );
    }
}
