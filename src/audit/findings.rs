use std::collections::BTreeMap;

use crate::audit::model::{AuditData, Finding, Recommendation, Severity};

/// Aggregate findings + recommendations from every populated section
/// of `AuditData`. Pure function; does not mutate the input.
///
/// Findings already emitted by individual analyzers (e.g. the
/// detection-gate Finding produced by `analyzers::safe_outputs`)
/// are PRESERVED — this function appends to whatever is already in
/// `audit.key_findings` / `audit.recommendations`, never replaces.
pub fn derive_findings(audit: &mut AuditData) {
    let mut findings = audit.key_findings.clone();
    let mut recommendations = audit.recommendations.clone();

    add_elevated_mcp_error_rate(audit, &mut findings, &mut recommendations);
    add_denied_network_domains(audit, &mut findings, &mut recommendations);
    add_high_token_usage(audit, &mut findings, &mut recommendations);
    add_repeated_noops(audit, &mut findings, &mut recommendations);
    add_missing_tools_cluster(audit, &mut findings, &mut recommendations);
    add_missing_data_cluster(audit, &mut findings, &mut recommendations);
    add_no_safe_outputs_proposed(audit, &mut findings, &mut recommendations);
    add_error_count_findings(audit, &mut findings, &mut recommendations);

    audit.key_findings = findings;
    audit.recommendations = recommendations;
}

fn add_elevated_mcp_error_rate(
    audit: &AuditData,
    findings: &mut Vec<Finding>,
    recommendations: &mut Vec<Recommendation>,
) {
    let Some(health) = &audit.mcp_server_health else {
        return;
    };

    for server in &health.servers {
        if !server.unreliable {
            continue;
        }

        let finding = Finding {
            category: String::from("mcp"),
            severity: Severity::High,
            title: format!("MCP server '{}' is unreliable", server.name),
            description: format!(
                "{} has an error rate of {:.1}% across {} calls (threshold: 10% with ≥5 calls).",
                server.name,
                server.error_rate * 100.0,
                server.total_calls
            ),
            impact: Some(format!(
                "Agent calls to {} may fail intermittently; consider retry or fallback strategy.",
                server.name
            )),
        };
        push_finding(findings, finding);

        let recommendation = Recommendation {
            priority: String::from("high"),
            action: format!("Inspect MCPG logs for {}", server.name),
            reason: String::from("Recurring errors degrade agent reliability."),
            example: None,
        };
        push_recommendation(recommendations, recommendation);
    }
}

fn add_denied_network_domains(
    audit: &AuditData,
    findings: &mut Vec<Finding>,
    recommendations: &mut Vec<Recommendation>,
) {
    let Some(firewall) = &audit.firewall_analysis else {
        return;
    };
    if firewall.denied_count == 0 {
        return;
    }

    let mut denied_domains = BTreeMap::<String, u64>::new();
    for domain in &firewall.domains {
        let domain_name = domain.domain.trim();
        if domain_name.is_empty() {
            continue;
        }

        let status = domain.status.trim().to_ascii_lowercase();
        if status == "allowed" {
            continue;
        }

        *denied_domains.entry(domain_name.to_string()).or_default() += domain.request_count;
    }

    if denied_domains.is_empty() {
        return;
    }

    let mut denied_domains = denied_domains.into_iter().collect::<Vec<_>>();
    denied_domains.sort_by(|(left_domain, left_count), (right_domain, right_count)| {
        right_count
            .cmp(left_count)
            .then_with(|| left_domain.cmp(right_domain))
    });

    let additional_domains = denied_domains.len().saturating_sub(5);
    for (index, (domain, request_count)) in denied_domains.into_iter().take(5).enumerate() {
        let finding = Finding {
            category: String::from("network"),
            severity: if index == 0 {
                Severity::High
            } else {
                Severity::Medium
            },
            title: format!("Network access denied: {domain}"),
            description: format!("AWF firewall denied {request_count} request(s) to {domain}."),
            impact: None,
        };
        push_finding(findings, finding);

        let recommendation = Recommendation {
            priority: String::from("medium"),
            action: format!("Add '{domain}' to network.allowed if access is legitimate."),
            reason: String::from(
                "Denied requests suggest the workflow may need an expanded allowlist.",
            ),
            example: None,
        };
        push_recommendation(recommendations, recommendation);
    }

    if additional_domains > 0 {
        push_finding(
            findings,
            Finding {
                category: String::from("network"),
                severity: Severity::Medium,
                title: format!("{additional_domains} additional domains were denied"),
                description: format!(
                    "AWF firewall denied requests to {additional_domains} additional domain(s) beyond the top 5 offenders."
                ),
                impact: None,
            },
        );
    }
}

fn add_high_token_usage(
    audit: &AuditData,
    findings: &mut Vec<Finding>,
    recommendations: &mut Vec<Recommendation>,
) {
    if audit.metrics.token_usage <= 100_000 {
        return;
    }

    let finding = Finding {
        category: String::from("cost"),
        severity: Severity::Medium,
        title: format!("High token usage: {} tokens", audit.metrics.token_usage),
        description: format!(
            "Run consumed {} tokens across {} turn(s). Consider reducing context size or pruning history.",
            audit.metrics.token_usage, audit.metrics.turns
        ),
        impact: None,
    };
    push_finding(findings, finding);

    let recommendation = Recommendation {
        priority: String::from("medium"),
        action: String::from("Review the agent prompt and tool usage for unnecessary context."),
        reason: String::from("Large prompts and long histories increase cost and latency."),
        example: None,
    };
    push_recommendation(recommendations, recommendation);
}

fn add_repeated_noops(
    audit: &AuditData,
    findings: &mut Vec<Finding>,
    recommendations: &mut Vec<Recommendation>,
) {
    let noop_count = audit.noops.len();
    if noop_count < 3 {
        return;
    }

    let finding = Finding {
        category: String::from("agent_behavior"),
        severity: Severity::Low,
        title: format!("Agent reported {} noop outcomes", noop_count),
        description: format!(
            "The agent declined to act {} time(s) without proposing safe outputs. Frequent noops may indicate prompt or tool gaps.",
            noop_count
        ),
        impact: None,
    };
    push_finding(findings, finding);

    let recommendation = Recommendation {
        priority: String::from("low"),
        action: String::from(
            "Review noop reasons to determine whether tools or prompt need adjustment.",
        ),
        reason: String::from(
            "Repeated noop outcomes often signal unclear instructions or missing capabilities.",
        ),
        example: None,
    };
    push_recommendation(recommendations, recommendation);
}

fn add_missing_tools_cluster(
    audit: &AuditData,
    findings: &mut Vec<Finding>,
    recommendations: &mut Vec<Recommendation>,
) {
    if audit.missing_tools.len() < 2 {
        return;
    }

    let mut grouped = BTreeMap::<String, Vec<String>>::new();
    for report in &audit.missing_tools {
        let Some(tool) = report.tool.as_deref() else {
            continue;
        };
        let tool = tool.trim();
        if tool.is_empty() {
            continue;
        }

        let reasons = grouped.entry(tool.to_string()).or_default();
        if let Some(reason) = report.reason.as_deref() {
            let reason = reason.trim();
            if !reason.is_empty() {
                reasons.push(reason.to_string());
            }
        }
    }

    for (tool, reasons) in grouped {
        let occurrence_count = audit
            .missing_tools
            .iter()
            .filter(|report| report.tool.as_deref().map(str::trim) == Some(tool.as_str()))
            .count();
        if occurrence_count < 2 {
            continue;
        }

        let reasons_summary = if reasons.is_empty() {
            String::from("none provided")
        } else {
            reasons.into_iter().take(3).collect::<Vec<_>>().join("; ")
        };

        let finding = Finding {
            category: String::from("tooling"),
            severity: Severity::Medium,
            title: format!("Tool '{tool}' marked missing {occurrence_count} times"),
            description: format!(
                "The agent attempted to use '{tool}' but it was not available. Reasons reported: {reasons_summary}."
            ),
            impact: None,
        };
        push_finding(findings, finding);

        let recommendation = Recommendation {
            priority: String::from("medium"),
            action: format!(
                "Add '{tool}' to the agent's tools: section if the use case is legitimate."
            ),
            reason: String::from(
                "Repeated missing-tool reports suggest a tooling configuration gap.",
            ),
            example: None,
        };
        push_recommendation(recommendations, recommendation);
    }
}

fn add_missing_data_cluster(
    audit: &AuditData,
    findings: &mut Vec<Finding>,
    _recommendations: &mut Vec<Recommendation>,
) {
    let missing_data_count = audit.missing_data.len();
    if missing_data_count < 3 {
        return;
    }

    push_finding(
        findings,
        Finding {
            category: String::from("inputs"),
            severity: Severity::Low,
            title: format!("{missing_data_count} missing-data reports"),
            description: format!(
                "The agent reported missing data {missing_data_count} time(s). This often means an input file, work-item, or prior context was unavailable."
            ),
            impact: None,
        },
    );
}

fn add_no_safe_outputs_proposed(
    audit: &AuditData,
    findings: &mut Vec<Finding>,
    recommendations: &mut Vec<Recommendation>,
) {
    let Some(summary) = &audit.safe_output_summary else {
        return;
    };
    if summary.proposed_count != 0 || !audit.noops.is_empty() {
        return;
    }

    let finding = Finding {
        category: String::from("runtime"),
        severity: Severity::Info,
        title: String::from("Agent produced no safe outputs and no noop record"),
        description: String::from(
            "The agent completed without proposing any safe outputs and without using the noop tool. This may indicate an early exit, an unhandled error, or a misconfigured agent.",
        ),
        impact: None,
    };
    push_finding(findings, finding);

    let recommendation = Recommendation {
        priority: String::from("medium"),
        action: String::from("Check the agent stdout (agent-output.txt) for early-exit signals."),
        reason: String::from(
            "Without outputs or an explicit noop, the raw agent log is the fastest way to explain the silent run.",
        ),
        example: None,
    };
    push_recommendation(recommendations, recommendation);
}

fn add_error_count_findings(
    audit: &AuditData,
    findings: &mut Vec<Finding>,
    _recommendations: &mut Vec<Recommendation>,
) {
    if audit.metrics.error_count == 0 {
        return;
    }

    push_finding(
        findings,
        Finding {
            category: String::from("runtime"),
            severity: Severity::Medium,
            title: format!("{} error(s) detected", audit.metrics.error_count),
            description: format!(
                "Audit detected {} error event(s) in the run logs.",
                audit.metrics.error_count
            ),
            impact: None,
        },
    );
}

fn push_finding(findings: &mut Vec<Finding>, finding: Finding) {
    if !findings.contains(&finding) {
        findings.push(finding);
    }
}

fn push_recommendation(recommendations: &mut Vec<Recommendation>, recommendation: Recommendation) {
    if !recommendations.contains(&recommendation) {
        recommendations.push(recommendation);
    }
}

#[cfg(test)]
mod tests {
    use super::derive_findings;
    use crate::audit::model::{
        AuditData, DomainStat, Finding, FirewallAnalysis, MCPServerHealth, MCPServerStats,
        MetricsData, MissingDataReport, MissingToolReport, NoopReport, Recommendation,
        SafeOutputSummary, Severity,
    };

    fn finding_by_title<'a>(audit: &'a AuditData, title: &str) -> &'a Finding {
        audit
            .key_findings
            .iter()
            .find(|finding| finding.title == title)
            .unwrap_or_else(|| panic!("missing finding: {title}"))
    }

    fn recommendation_by_action<'a>(audit: &'a AuditData, action: &str) -> &'a Recommendation {
        audit
            .recommendations
            .iter()
            .find(|recommendation| recommendation.action == action)
            .unwrap_or_else(|| panic!("missing recommendation: {action}"))
    }

    #[test]
    fn empty_audit_adds_no_findings_or_recommendations() {
        let mut audit = AuditData::default();

        derive_findings(&mut audit);

        assert!(audit.key_findings.is_empty());
        assert!(audit.recommendations.is_empty());
    }

    #[test]
    fn detection_finding_is_preserved_without_duplication() {
        let detection_finding = Finding {
            category: String::from("safe_outputs"),
            severity: Severity::High,
            title: String::from("Detection rejected 2 safe output(s)"),
            description: String::from("The threat-analysis verdict dropped the batch."),
            impact: Some(String::from("No items were created.")),
        };
        let mut audit = AuditData {
            key_findings: vec![detection_finding.clone()],
            ..Default::default()
        };

        derive_findings(&mut audit);

        assert!(audit.key_findings.contains(&detection_finding));
        assert_eq!(
            audit
                .key_findings
                .iter()
                .filter(|finding| finding.title == detection_finding.title)
                .count(),
            1
        );
    }

    #[test]
    fn elevated_mcp_error_rate_rule_emits_finding_and_recommendation() {
        let mut audit = AuditData {
            mcp_server_health: Some(MCPServerHealth {
                servers: vec![MCPServerStats {
                    name: String::from("github"),
                    total_calls: 10,
                    error_count: 2,
                    error_rate: 0.2,
                    unreliable: true,
                }],
            }),
            ..Default::default()
        };

        derive_findings(&mut audit);

        let finding = finding_by_title(&audit, "MCP server 'github' is unreliable");
        assert_eq!(finding.severity, Severity::High);
        assert_eq!(
            finding.description,
            "github has an error rate of 20.0% across 10 calls (threshold: 10% with ≥5 calls)."
        );
        assert_eq!(
            finding.impact.as_deref(),
            Some(
                "Agent calls to github may fail intermittently; consider retry or fallback strategy."
            )
        );

        let recommendation = recommendation_by_action(&audit, "Inspect MCPG logs for github");
        assert_eq!(recommendation.priority, "high");
        assert_eq!(
            recommendation.reason,
            "Recurring errors degrade agent reliability."
        );
    }

    #[test]
    fn denied_network_domains_rule_emits_finding_and_recommendation() {
        let mut audit = AuditData {
            firewall_analysis: Some(FirewallAnalysis {
                domains: vec![DomainStat {
                    domain: String::from("example.com"),
                    status: String::from("denied"),
                    request_count: 3,
                    ..Default::default()
                }],
                denied_count: 3,
                ..Default::default()
            }),
            ..Default::default()
        };

        derive_findings(&mut audit);

        let finding = finding_by_title(&audit, "Network access denied: example.com");
        assert_eq!(finding.severity, Severity::High);
        assert_eq!(
            finding.description,
            "AWF firewall denied 3 request(s) to example.com."
        );

        let recommendation = recommendation_by_action(
            &audit,
            "Add 'example.com' to network.allowed if access is legitimate.",
        );
        assert_eq!(recommendation.priority, "medium");
    }

    #[test]
    fn high_token_usage_rule_emits_finding_and_recommendation() {
        let mut audit = AuditData {
            metrics: MetricsData {
                token_usage: 100_001,
                turns: 7,
                ..Default::default()
            },
            ..Default::default()
        };

        derive_findings(&mut audit);

        let finding = finding_by_title(&audit, "High token usage: 100001 tokens");
        assert_eq!(finding.severity, Severity::Medium);
        assert_eq!(
            finding.description,
            "Run consumed 100001 tokens across 7 turn(s). Consider reducing context size or pruning history."
        );

        let recommendation = recommendation_by_action(
            &audit,
            "Review the agent prompt and tool usage for unnecessary context.",
        );
        assert_eq!(recommendation.priority, "medium");
    }

    #[test]
    fn repeated_noops_rule_emits_finding_and_recommendation() {
        let mut audit = AuditData {
            noops: vec![
                NoopReport::default(),
                NoopReport::default(),
                NoopReport::default(),
            ],
            ..Default::default()
        };

        derive_findings(&mut audit);

        let finding = finding_by_title(&audit, "Agent reported 3 noop outcomes");
        assert_eq!(finding.severity, Severity::Low);
        assert_eq!(
            finding.description,
            "The agent declined to act 3 time(s) without proposing safe outputs. Frequent noops may indicate prompt or tool gaps."
        );

        let recommendation = recommendation_by_action(
            &audit,
            "Review noop reasons to determine whether tools or prompt need adjustment.",
        );
        assert_eq!(recommendation.priority, "low");
    }

    #[test]
    fn missing_tools_cluster_rule_emits_finding_and_recommendation() {
        let mut audit = AuditData {
            missing_tools: vec![
                MissingToolReport {
                    tool: Some(String::from("azure-devops")),
                    reason: Some(String::from("tool not configured")),
                    ..Default::default()
                },
                MissingToolReport {
                    tool: Some(String::from("azure-devops")),
                    reason: Some(String::from("tool omitted from workflow")),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        derive_findings(&mut audit);

        let finding = finding_by_title(&audit, "Tool 'azure-devops' marked missing 2 times");
        assert_eq!(finding.severity, Severity::Medium);
        assert_eq!(
            finding.description,
            "The agent attempted to use 'azure-devops' but it was not available. Reasons reported: tool not configured; tool omitted from workflow."
        );

        let recommendation = recommendation_by_action(
            &audit,
            "Add 'azure-devops' to the agent's tools: section if the use case is legitimate.",
        );
        assert_eq!(recommendation.priority, "medium");
    }

    #[test]
    fn missing_data_cluster_rule_emits_finding_only() {
        let mut audit = AuditData {
            missing_data: vec![
                MissingDataReport::default(),
                MissingDataReport::default(),
                MissingDataReport::default(),
            ],
            ..Default::default()
        };

        derive_findings(&mut audit);

        let finding = finding_by_title(&audit, "3 missing-data reports");
        assert_eq!(finding.severity, Severity::Low);
        assert_eq!(
            finding.description,
            "The agent reported missing data 3 time(s). This often means an input file, work-item, or prior context was unavailable."
        );
        assert!(audit.recommendations.is_empty());
    }

    #[test]
    fn no_safe_outputs_proposed_rule_emits_finding_and_recommendation() {
        let mut audit = AuditData {
            safe_output_summary: Some(SafeOutputSummary::default()),
            ..Default::default()
        };

        derive_findings(&mut audit);

        let finding = finding_by_title(&audit, "Agent produced no safe outputs and no noop record");
        assert_eq!(finding.severity, Severity::Info);
        assert_eq!(
            finding.description,
            "The agent completed without proposing any safe outputs and without using the noop tool. This may indicate an early exit, an unhandled error, or a misconfigured agent."
        );

        let recommendation = recommendation_by_action(
            &audit,
            "Check the agent stdout (agent-output.txt) for early-exit signals.",
        );
        assert_eq!(recommendation.priority, "medium");
    }

    #[test]
    fn error_count_rule_emits_finding_only() {
        let mut audit = AuditData {
            metrics: MetricsData {
                error_count: 2,
                ..Default::default()
            },
            ..Default::default()
        };

        derive_findings(&mut audit);

        let finding = finding_by_title(&audit, "2 error(s) detected");
        assert_eq!(finding.severity, Severity::Medium);
        assert_eq!(
            finding.description,
            "Audit detected 2 error event(s) in the run logs."
        );
        assert!(audit.recommendations.is_empty());
    }

    #[test]
    fn combined_findings_are_appended_and_preserved_across_passes() {
        let mut audit = AuditData {
            mcp_server_health: Some(MCPServerHealth {
                servers: vec![MCPServerStats {
                    name: String::from("cache-memory"),
                    total_calls: 8,
                    error_count: 2,
                    error_rate: 0.25,
                    unreliable: true,
                }],
            }),
            firewall_analysis: Some(FirewallAnalysis {
                domains: vec![
                    DomainStat {
                        domain: String::from("packages.example.com"),
                        status: String::from("denied"),
                        request_count: 6,
                        ..Default::default()
                    },
                    DomainStat {
                        domain: String::from("artifacts.example.com"),
                        status: String::from("mixed"),
                        request_count: 4,
                        ..Default::default()
                    },
                ],
                denied_count: 10,
                ..Default::default()
            }),
            safe_output_summary: Some(SafeOutputSummary::default()),
            ..Default::default()
        };

        derive_findings(&mut audit);
        audit.noops = vec![
            NoopReport::default(),
            NoopReport::default(),
            NoopReport::default(),
        ];
        derive_findings(&mut audit);

        assert_eq!(audit.key_findings.len(), 5);
        assert!(
            audit
                .key_findings
                .iter()
                .any(|finding| finding.title == "MCP server 'cache-memory' is unreliable")
        );
        assert!(
            audit
                .key_findings
                .iter()
                .any(|finding| finding.title == "Network access denied: packages.example.com")
        );
        assert!(
            audit
                .key_findings
                .iter()
                .any(|finding| finding.title == "Network access denied: artifacts.example.com")
        );
        assert!(audit
            .key_findings
            .iter()
            .any(|finding| finding.title == "Agent produced no safe outputs and no noop record"));
        assert!(
            audit
                .key_findings
                .iter()
                .any(|finding| finding.title == "Agent reported 3 noop outcomes")
        );
    }

    #[test]
    fn denied_domains_rule_caps_detailed_findings_and_adds_rollup() {
        let mut audit = AuditData {
            firewall_analysis: Some(FirewallAnalysis {
                domains: vec![
                    DomainStat {
                        domain: String::from("d1.example.com"),
                        status: String::from("denied"),
                        request_count: 7,
                        ..Default::default()
                    },
                    DomainStat {
                        domain: String::from("d2.example.com"),
                        status: String::from("denied"),
                        request_count: 6,
                        ..Default::default()
                    },
                    DomainStat {
                        domain: String::from("d3.example.com"),
                        status: String::from("denied"),
                        request_count: 5,
                        ..Default::default()
                    },
                    DomainStat {
                        domain: String::from("d4.example.com"),
                        status: String::from("denied"),
                        request_count: 4,
                        ..Default::default()
                    },
                    DomainStat {
                        domain: String::from("d5.example.com"),
                        status: String::from("denied"),
                        request_count: 3,
                        ..Default::default()
                    },
                    DomainStat {
                        domain: String::from("d6.example.com"),
                        status: String::from("denied"),
                        request_count: 2,
                        ..Default::default()
                    },
                    DomainStat {
                        domain: String::from("d7.example.com"),
                        status: String::from("denied"),
                        request_count: 1,
                        ..Default::default()
                    },
                ],
                denied_count: 28,
                ..Default::default()
            }),
            ..Default::default()
        };

        derive_findings(&mut audit);

        assert_eq!(audit.key_findings.len(), 6);
        assert!(
            audit
                .key_findings
                .iter()
                .any(|finding| finding.title == "Network access denied: d1.example.com")
        );
        assert!(
            audit
                .key_findings
                .iter()
                .any(|finding| finding.title == "Network access denied: d5.example.com")
        );
        assert!(
            audit
                .key_findings
                .iter()
                .any(|finding| finding.title == "2 additional domains were denied")
        );
        assert_eq!(audit.recommendations.len(), 5);
    }
}
