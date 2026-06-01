//! AWF firewall log analyzer for `ado-aw audit`.

use anyhow::Context;
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::Path;

use crate::audit::model::{DomainStat, FirewallAnalysis};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Allowed,
    Denied,
    Unknown,
}

#[derive(Debug, Default)]
struct DomainAccumulator {
    request_count: u64,
    allowed_count: u64,
    denied_count: u64,
    first_seen: Option<String>,
    last_seen: Option<String>,
}

impl DomainAccumulator {
    fn record(&mut self, verdict: Verdict, timestamp: Option<&str>) {
        self.request_count += 1;
        match verdict {
            Verdict::Allowed => self.allowed_count += 1,
            Verdict::Denied => self.denied_count += 1,
            Verdict::Unknown => {}
        }
        update_first_seen(&mut self.first_seen, timestamp);
        update_last_seen(&mut self.last_seen, timestamp);
    }

    fn status(&self) -> String {
        if self.request_count > 0 && self.allowed_count == self.request_count {
            "allowed".to_string()
        } else if self.request_count > 0 && self.denied_count == self.request_count {
            "denied".to_string()
        } else {
            "mixed".to_string()
        }
    }
}

/// Analyze AWF firewall logs in `<agent_outputs>/logs/firewall/`.
///
/// Scans every `*.jsonl` / `*.log` file in the directory, parses each
/// line as JSON, and aggregates per-domain request counts + allow/deny
/// verdicts.
///
/// Returns `Ok(None)` if the directory does not exist (the agent ran
/// without AWF, e.g. on a target that does not network-isolate).
/// Returns `Ok(Some(empty))` if the directory exists but contains no
/// recognisable entries — surfacing the empty state lets the audit
/// renderer distinguish "AWF disabled" from "AWF ran but logged nothing".
pub async fn analyze_firewall_logs(
    firewall_logs_dir: &std::path::Path,
) -> anyhow::Result<Option<crate::audit::model::FirewallAnalysis>> {
    match tokio::fs::metadata(firewall_logs_dir).await {
        Ok(metadata) => {
            anyhow::ensure!(
                metadata.is_dir(),
                "Firewall logs path is not a directory: {}",
                firewall_logs_dir.display()
            );
        }
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed to stat {}", firewall_logs_dir.display()));
        }
    }

    let mut totals = FirewallAnalysis::default();
    let mut per_domain = BTreeMap::<String, DomainAccumulator>::new();
    let mut entries = tokio::fs::read_dir(firewall_logs_dir)
        .await
        .with_context(|| format!("Failed to read {}", firewall_logs_dir.display()))?;

    while let Some(entry) = entries
        .next_entry()
        .await
        .with_context(|| format!("Failed to iterate {}", firewall_logs_dir.display()))?
    {
        let file_type = entry
            .file_type()
            .await
            .with_context(|| format!("Failed to inspect {}", entry.path().display()))?;
        if !file_type.is_file() {
            continue;
        }

        let path = entry.path();
        if !is_firewall_log_file(&path) {
            continue;
        }

        let contents = tokio::fs::read_to_string(&path)
            .await
            .with_context(|| format!("Failed to read firewall log {}", path.display()))?;

        for line in contents.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            let value: Value = match serde_json::from_str(trimmed) {
                Ok(value) => value,
                Err(_) => continue,
            };

            let Some(domain) = extract_string_field(&value, &["host", "domain", "hostname"]) else {
                continue;
            };

            let verdict = extract_verdict(&value);
            let timestamp = extract_string_field(&value, &["timestamp", "time", "@timestamp"]);

            let domain_entry = per_domain.entry(domain).or_default();
            domain_entry.record(verdict, timestamp.as_deref());

            match verdict {
                Verdict::Allowed => totals.allowed_count += 1,
                Verdict::Denied => totals.denied_count += 1,
                Verdict::Unknown => {}
            }
        }
    }

    totals.domains = per_domain
        .into_iter()
        .map(|(domain, stats)| DomainStat {
            domain,
            status: stats.status(),
            request_count: stats.request_count,
            first_seen: stats.first_seen,
            last_seen: stats.last_seen,
        })
        .collect();

    totals.total_requests = totals
        .domains
        .iter()
        .map(|domain| domain.request_count)
        .sum();
    totals.domains.sort_by(|left, right| {
        right
            .request_count
            .cmp(&left.request_count)
            .then_with(|| left.domain.cmp(&right.domain))
    });

    Ok(Some(totals))
}

fn is_firewall_log_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            extension.eq_ignore_ascii_case("jsonl") || extension.eq_ignore_ascii_case("log")
        })
        .unwrap_or(false)
}

fn extract_string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .filter_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .find(|candidate| !candidate.is_empty())
        .map(ToOwned::to_owned)
}

fn extract_verdict(value: &Value) -> Verdict {
    let Some(raw_verdict) = extract_string_field(value, &["verdict", "status", "action"]) else {
        return Verdict::Unknown;
    };

    match raw_verdict.to_ascii_lowercase().as_str() {
        "allow" | "allowed" | "pass" => Verdict::Allowed,
        "deny" | "denied" | "block" | "blocked" => Verdict::Denied,
        _ => Verdict::Unknown,
    }
}

fn update_first_seen(current: &mut Option<String>, candidate: Option<&str>) {
    let Some(candidate) = candidate.filter(|candidate| !candidate.is_empty()) else {
        return;
    };

    match current {
        Some(existing) if existing.as_str() <= candidate => {}
        _ => *current = Some(candidate.to_string()),
    }
}

fn update_last_seen(current: &mut Option<String>, candidate: Option<&str>) {
    let Some(candidate) = candidate.filter(|candidate| !candidate.is_empty()) else {
        return;
    };

    match current {
        Some(existing) if existing.as_str() >= candidate => {}
        _ => *current = Some(candidate.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn expected_mixed_analysis() -> FirewallAnalysis {
        FirewallAnalysis {
            domains: vec![
                DomainStat {
                    domain: "api.github.com".to_string(),
                    status: "allowed".to_string(),
                    request_count: 2,
                    first_seen: Some("2026-01-01T00:00:00Z".to_string()),
                    last_seen: Some("2026-01-01T00:00:02Z".to_string()),
                },
                DomainStat {
                    domain: "evil.example.com".to_string(),
                    status: "denied".to_string(),
                    request_count: 2,
                    first_seen: Some("2026-01-01T00:00:01Z".to_string()),
                    last_seen: Some("2026-01-01T00:00:03Z".to_string()),
                },
                DomainStat {
                    domain: "unknown-verdict.example".to_string(),
                    status: "mixed".to_string(),
                    request_count: 1,
                    first_seen: Some("2026-01-01T00:00:04Z".to_string()),
                    last_seen: Some("2026-01-01T00:00:04Z".to_string()),
                },
            ],
            total_requests: 5,
            allowed_count: 2,
            denied_count: 2,
        }
    }

    fn mixed_fixture_lines() -> &'static str {
        concat!(
            "{\"timestamp\":\"2026-01-01T00:00:00Z\",\"host\":\"api.github.com\",\"verdict\":\"allow\",\"method\":\"GET\",\"url\":\"https://api.github.com/repos/o/r\",\"status_code\":200}\n",
            "{\"timestamp\":\"2026-01-01T00:00:02Z\",\"host\":\"api.github.com\",\"verdict\":\"allowed\",\"method\":\"GET\",\"url\":\"https://api.github.com/user\",\"status_code\":200}\n",
            "{\"timestamp\":\"2026-01-01T00:00:01Z\",\"host\":\"evil.example.com\",\"verdict\":\"deny\",\"method\":\"CONNECT\",\"url\":\"https://evil.example.com\",\"status_code\":403}\n",
            "{\"timestamp\":\"2026-01-01T00:00:03Z\",\"host\":\"evil.example.com\",\"verdict\":\"blocked\",\"method\":\"CONNECT\",\"url\":\"https://evil.example.com/admin\",\"status_code\":403}\n",
            "{\"timestamp\":\"2026-01-01T00:00:04Z\",\"host\":\"unknown-verdict.example\",\"verdict\":\"mystery\",\"method\":\"GET\",\"url\":\"https://unknown-verdict.example\",\"status_code\":200}\n"
        )
    }

    async fn write_log_file(dir: &Path, name: &str, contents: &str) {
        tokio::fs::create_dir_all(dir).await.unwrap();
        tokio::fs::write(dir.join(name), contents).await.unwrap();
    }

    #[tokio::test]
    async fn returns_none_when_directory_absent() {
        let temp_dir = TempDir::new().unwrap();
        let firewall_dir = temp_dir.path().join("logs").join("firewall");

        let result = analyze_firewall_logs(&firewall_dir).await.unwrap();

        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn returns_empty_analysis_when_directory_exists_but_has_no_entries() {
        let temp_dir = TempDir::new().unwrap();
        let firewall_dir = temp_dir.path().join("logs").join("firewall");
        tokio::fs::create_dir_all(&firewall_dir).await.unwrap();

        let result = analyze_firewall_logs(&firewall_dir).await.unwrap();

        assert_eq!(result, Some(FirewallAnalysis::default()));
    }

    #[tokio::test]
    async fn aggregates_mixed_log_fixture() {
        let temp_dir = TempDir::new().unwrap();
        let firewall_dir = temp_dir.path().join("logs").join("firewall");
        write_log_file(&firewall_dir, "firewall.jsonl", mixed_fixture_lines()).await;

        let result = analyze_firewall_logs(&firewall_dir).await.unwrap();

        assert_eq!(result, Some(expected_mixed_analysis()));
    }

    #[tokio::test]
    async fn aggregates_across_multiple_files() {
        let temp_dir = TempDir::new().unwrap();
        let firewall_dir = temp_dir.path().join("logs").join("firewall");
        write_log_file(
            &firewall_dir,
            "firewall-001.jsonl",
            concat!(
                "{\"timestamp\":\"2026-01-01T00:00:00Z\",\"host\":\"api.github.com\",\"verdict\":\"allow\"}\n",
                "{\"timestamp\":\"2026-01-01T00:00:02Z\",\"host\":\"api.github.com\",\"verdict\":\"allowed\"}\n",
                "{\"timestamp\":\"2026-01-01T00:00:01Z\",\"host\":\"evil.example.com\",\"verdict\":\"deny\"}\n"
            ),
        )
        .await;
        write_log_file(
            &firewall_dir,
            "firewall-002.jsonl",
            concat!(
                "{\"timestamp\":\"2026-01-01T00:00:03Z\",\"host\":\"evil.example.com\",\"verdict\":\"blocked\"}\n",
                "{\"timestamp\":\"2026-01-01T00:00:04Z\",\"host\":\"unknown-verdict.example\",\"verdict\":\"mystery\"}\n"
            ),
        )
        .await;

        let result = analyze_firewall_logs(&firewall_dir).await.unwrap();

        assert_eq!(result, Some(expected_mixed_analysis()));
    }

    #[tokio::test]
    async fn uses_field_name_fallbacks() {
        let temp_dir = TempDir::new().unwrap();
        let firewall_dir = temp_dir.path().join("logs").join("firewall");
        write_log_file(
            &firewall_dir,
            "fallbacks.log",
            "{\"time\":\"2026-01-01T00:00:05Z\",\"hostname\":\"packages.example.org\",\"action\":\"block\"}\n",
        )
        .await;

        let result = analyze_firewall_logs(&firewall_dir).await.unwrap();

        assert_eq!(
            result,
            Some(FirewallAnalysis {
                domains: vec![DomainStat {
                    domain: "packages.example.org".to_string(),
                    status: "denied".to_string(),
                    request_count: 1,
                    first_seen: Some("2026-01-01T00:00:05Z".to_string()),
                    last_seen: Some("2026-01-01T00:00:05Z".to_string()),
                }],
                total_requests: 1,
                allowed_count: 0,
                denied_count: 1,
            })
        );
    }

    #[tokio::test]
    async fn skips_malformed_lines() {
        let temp_dir = TempDir::new().unwrap();
        let firewall_dir = temp_dir.path().join("logs").join("firewall");
        write_log_file(
            &firewall_dir,
            "malformed.jsonl",
            concat!(
                "not-json\n",
                "{\"timestamp\":\"2026-01-01T00:00:06Z\",\"host\":\"api.github.com\",\"verdict\":\"allow\"}\n"
            ),
        )
        .await;

        let result = analyze_firewall_logs(&firewall_dir).await.unwrap();

        assert_eq!(
            result,
            Some(FirewallAnalysis {
                domains: vec![DomainStat {
                    domain: "api.github.com".to_string(),
                    status: "allowed".to_string(),
                    request_count: 1,
                    first_seen: Some("2026-01-01T00:00:06Z".to_string()),
                    last_seen: Some("2026-01-01T00:00:06Z".to_string()),
                }],
                total_requests: 1,
                allowed_count: 1,
                denied_count: 0,
            })
        );
    }
}
