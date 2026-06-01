//! AWF policy artifact analyzer for `ado-aw audit`.

use anyhow::{Context, Result};
use log::warn;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::audit::model::{PolicyAnalysis, PolicyRule};

const MANIFEST_FILENAME: &str = "policy-manifest.json";
const AUDIT_FILENAME: &str = "audit.jsonl";
const UNMATCHED_PATTERN: &str = "(unmatched)";
const UNKNOWN_VERDICT: &str = "(unknown)";

/// Analyze AWF policy artifacts in `<agent_outputs>/logs/firewall/`.
///
/// Reads `policy-manifest.json` for the static rule list and `audit.jsonl`
/// for per-rule hit counts. Either file may be absent — the analyzer
/// returns `Ok(None)` only when BOTH are absent. When only one is
/// present, it produces a partial result (rules with zero hits, or
/// hit counts against synthetic unmatched rules).
pub async fn analyze_policy(firewall_logs_dir: &Path) -> Result<Option<PolicyAnalysis>> {
    let manifest = read_policy_manifest(&firewall_logs_dir.join(MANIFEST_FILENAME)).await?;
    let audit_hits = read_policy_audit(&firewall_logs_dir.join(AUDIT_FILENAME)).await?;

    if manifest.is_none() && audit_hits.is_none() {
        return Ok(None);
    }

    let mut policies = manifest.unwrap_or_default();
    let mut unmatched_hits: BTreeMap<String, u64> = BTreeMap::new();

    if let Some(audit_hits) = audit_hits {
        for (rule_pattern, line_verdict) in audit_hits {
            if let Some(policy) = policies
                .iter_mut()
                .find(|policy| policy.pattern == rule_pattern)
            {
                policy.hit_count += 1;
            } else {
                *unmatched_hits.entry(line_verdict).or_default() += 1;
            }
        }
    }

    for (verdict, hit_count) in unmatched_hits {
        policies.push(PolicyRule {
            pattern: String::from(UNMATCHED_PATTERN),
            verdict,
            hit_count,
        });
    }

    policies.sort_by(|left, right| {
        right
            .hit_count
            .cmp(&left.hit_count)
            .then_with(|| left.pattern.cmp(&right.pattern))
            .then_with(|| left.verdict.cmp(&right.verdict))
    });

    let allow_count = policies
        .iter()
        .filter(|policy| normalize_verdict(&policy.verdict) == Some(NormalizedVerdict::Allowed))
        .map(|policy| policy.hit_count)
        .sum();
    let deny_count = policies
        .iter()
        .filter(|policy| normalize_verdict(&policy.verdict) == Some(NormalizedVerdict::Denied))
        .map(|policy| policy.hit_count)
        .sum();

    Ok(Some(PolicyAnalysis {
        policies,
        allow_count,
        deny_count,
    }))
}

async fn read_policy_manifest(path: &Path) -> Result<Option<Vec<PolicyRule>>> {
    let contents = match tokio::fs::read_to_string(path).await {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed to read policy manifest: {}", path.display()));
        }
    };

    let manifest: Value = match serde_json::from_str(&contents) {
        Ok(manifest) => manifest,
        Err(error) => {
            warn!(
                "Failed to parse policy manifest '{}': {}",
                path.display(),
                error
            );
            return Ok(None);
        }
    };

    let mut policies = Vec::new();
    if let Some(rules) = manifest.get("rules").and_then(Value::as_array) {
        for rule in rules {
            let Some(pattern) = extract_string(rule, &["pattern", "host", "domain"]) else {
                continue;
            };

            policies.push(PolicyRule {
                pattern,
                verdict: extract_string(rule, &["verdict", "action", "status"]).unwrap_or_default(),
                hit_count: 0,
            });
        }
    }

    Ok(Some(policies))
}

async fn read_policy_audit(path: &Path) -> Result<Option<Vec<(String, String)>>> {
    let file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed to open policy audit log: {}", path.display()));
        }
    };

    let reader = BufReader::new(file);
    let mut lines = reader.lines();
    let mut hits = Vec::new();

    while let Some(line) = lines
        .next_line()
        .await
        .with_context(|| format!("Failed to read policy audit log: {}", path.display()))?
    {
        if line.trim().is_empty() {
            continue;
        }

        let Ok(record) = serde_json::from_str::<Value>(&line) else {
            continue;
        };

        let Some(rule_pattern) = extract_string(&record, &["rule", "pattern", "host", "domain"])
        else {
            continue;
        };

        let verdict = extract_string(&record, &["verdict", "action", "status"])
            .unwrap_or_else(|| String::from(UNKNOWN_VERDICT));
        hits.push((rule_pattern, verdict));
    }

    Ok(Some(hits))
}

fn extract_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .filter_map(|key| value.get(*key))
        .find_map(value_to_string)
}

fn value_to_string(value: &Value) -> Option<String> {
    let text = match value {
        Value::String(text) => text.trim().to_owned(),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        _ => return None,
    };

    if text.is_empty() { None } else { Some(text) }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NormalizedVerdict {
    Allowed,
    Denied,
}

fn normalize_verdict(verdict: &str) -> Option<NormalizedVerdict> {
    match verdict.trim().to_ascii_lowercase().as_str() {
        "allow" | "allowed" | "pass" => Some(NormalizedVerdict::Allowed),
        "deny" | "denied" | "block" | "blocked" => Some(NormalizedVerdict::Denied),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_fixture(dir: &TempDir, name: &str, contents: &str) {
        std::fs::write(dir.path().join(name), contents).expect("write fixture");
    }

    fn find_policy<'a>(
        analysis: &'a PolicyAnalysis,
        pattern: &str,
        verdict: &str,
    ) -> &'a PolicyRule {
        analysis
            .policies
            .iter()
            .find(|policy| policy.pattern == pattern && policy.verdict == verdict)
            .expect("policy should exist")
    }

    #[tokio::test]
    async fn returns_none_when_both_files_are_absent() {
        let dir = tempfile::tempdir().expect("tempdir");

        let analysis = analyze_policy(dir.path())
            .await
            .expect("analysis should succeed");

        assert_eq!(analysis, None);
    }

    #[tokio::test]
    async fn manifest_only_returns_zero_hit_rules() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_fixture(
            &dir,
            MANIFEST_FILENAME,
            r#"{
  "version": 1,
  "rules": [
    {"pattern": "z.example.com", "verdict": "deny"},
    {"host": "a.example.com", "action": "allow"}
  ]
}"#,
        );

        let analysis = analyze_policy(dir.path())
            .await
            .expect("analysis should succeed")
            .expect("analysis should exist");

        assert_eq!(analysis.allow_count, 0);
        assert_eq!(analysis.deny_count, 0);
        assert_eq!(
            analysis.policies,
            vec![
                PolicyRule {
                    pattern: String::from("a.example.com"),
                    verdict: String::from("allow"),
                    hit_count: 0,
                },
                PolicyRule {
                    pattern: String::from("z.example.com"),
                    verdict: String::from("deny"),
                    hit_count: 0,
                },
            ]
        );
    }

    #[tokio::test]
    async fn audit_only_returns_synthetic_unmatched_rules() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_fixture(
            &dir,
            AUDIT_FILENAME,
            r#"{"rule":"api.github.com","verdict":"allow"}
not json
{"rule":"evil.example.com","status":"deny"}
{"pattern":"another.example.com","action":"allow"}
"#,
        );

        let analysis = analyze_policy(dir.path())
            .await
            .expect("analysis should succeed")
            .expect("analysis should exist");

        assert_eq!(analysis.allow_count, 2);
        assert_eq!(analysis.deny_count, 1);
        assert_eq!(
            analysis.policies,
            vec![
                PolicyRule {
                    pattern: String::from(UNMATCHED_PATTERN),
                    verdict: String::from("allow"),
                    hit_count: 2,
                },
                PolicyRule {
                    pattern: String::from(UNMATCHED_PATTERN),
                    verdict: String::from("deny"),
                    hit_count: 1,
                },
            ]
        );
    }

    #[tokio::test]
    async fn combines_manifest_rules_and_audit_hits() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_fixture(
            &dir,
            MANIFEST_FILENAME,
            r#"{
  "version": 1,
  "rules": [
    {"pattern": "api.github.com", "verdict": "allow"},
    {"pattern": "*.azure.com", "verdict": "allow"},
    {"domain": "*", "status": "deny"}
  ]
}"#,
        );
        write_fixture(
            &dir,
            AUDIT_FILENAME,
            r#"{"timestamp":"2025-01-01T00:00:00Z","host":"api.github.com","rule":"api.github.com","verdict":"allow"}
{"timestamp":"2025-01-01T00:00:01Z","host":"foo.azure.com","rule":"*.azure.com","verdict":"allow"}
{"timestamp":"2025-01-01T00:00:02Z","host":"bar.azure.com","rule":"*.azure.com","verdict":"allow"}
{"timestamp":"2025-01-01T00:00:03Z","host":"api.github.com","rule":"api.github.com","verdict":"allow"}
{"timestamp":"2025-01-01T00:00:04Z","host":"evil.example.com","rule":"*","verdict":"deny"}
{"timestamp":"2025-01-01T00:00:05Z","host":"other.example.com","rule":"missing-allow","verdict":"allow"}
{"timestamp":"2025-01-01T00:00:06Z","host":"worse.example.com","rule":"missing-deny","status":"deny"}
"#,
        );

        let analysis = analyze_policy(dir.path())
            .await
            .expect("analysis should succeed")
            .expect("analysis should exist");

        assert_eq!(analysis.allow_count, 5);
        assert_eq!(analysis.deny_count, 2);
        assert_eq!(analysis.policies.len(), 5);
        assert_eq!(
            find_policy(&analysis, "api.github.com", "allow").hit_count,
            2
        );
        assert_eq!(find_policy(&analysis, "*.azure.com", "allow").hit_count, 2);
        assert_eq!(find_policy(&analysis, "*", "deny").hit_count, 1);
        assert_eq!(
            find_policy(&analysis, UNMATCHED_PATTERN, "allow").hit_count,
            1
        );
        assert_eq!(
            find_policy(&analysis, UNMATCHED_PATTERN, "deny").hit_count,
            1
        );
        assert_eq!(
            analysis
                .policies
                .iter()
                .map(|policy| (
                    policy.pattern.as_str(),
                    policy.verdict.as_str(),
                    policy.hit_count
                ))
                .collect::<Vec<_>>(),
            vec![
                ("*.azure.com", "allow", 2),
                ("api.github.com", "allow", 2),
                (UNMATCHED_PATTERN, "allow", 1),
                (UNMATCHED_PATTERN, "deny", 1),
                ("*", "deny", 1),
            ]
        );
    }

    #[tokio::test]
    async fn malformed_manifest_is_treated_as_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_fixture(&dir, MANIFEST_FILENAME, "not json");
        write_fixture(
            &dir,
            AUDIT_FILENAME,
            r#"{"rule":"missing-rule","verdict":"deny"}
"#,
        );

        let analysis = analyze_policy(dir.path())
            .await
            .expect("analysis should succeed")
            .expect("analysis should exist");

        assert_eq!(analysis.allow_count, 0);
        assert_eq!(analysis.deny_count, 1);
        assert_eq!(
            analysis.policies,
            vec![PolicyRule {
                pattern: String::from(UNMATCHED_PATTERN),
                verdict: String::from("deny"),
                hit_count: 1,
            }]
        );
    }
}
