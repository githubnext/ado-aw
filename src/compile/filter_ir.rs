//! Filter expression intermediate representation (IR).
//!
//! This module defines a typed IR for trigger filter expressions. The IR
//! separates **data acquisition** (what runtime facts to collect) from
//! **predicate evaluation** (what boolean tests to apply), enabling:
//!
//! - Compile-time conflict detection (impossible/redundant filter combos)
//! - Dependency-ordered fact acquisition (pipeline vars → API → computed)
//! - A single codegen pass from IR → bash gate step
//!
//! # Architecture
//!
//! ```text
//! PrFilters / PipelineFilters
//!         │
//!         ▼
//!   ┌──────────────┐
//!   │  1. Lower    │  Filters → Vec<FilterCheck>
//!   └──────┬───────┘
//!          │
//!          ▼
//!   ┌──────────────┐
//!   │  2. Validate │  Vec<FilterCheck> → Vec<Diagnostic>
//!   └──────┬───────┘
//!          │
//!          ▼
//!   ┌──────────────┐
//!   │  3. Codegen  │  GateContext + Vec<FilterCheck> → bash
//!   └──────────────┘
//! ```

use std::collections::BTreeSet;
use std::fmt;

// ─── Fact Sources ───────────────────────────────────────────────────────────

/// A typed runtime fact that can be acquired and referenced by predicates.
///
/// Each variant maps to a specific piece of data available at pipeline runtime,
/// with known acquisition cost (free pipeline variable vs. REST API call vs.
/// runtime computation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Fact {
    // ── Pipeline variables (free — always available) ────────────────────
    /// PR title: `$(System.PullRequest.Title)`
    PrTitle,
    /// Author email: `$(Build.RequestedForEmail)`
    AuthorEmail,
    /// PR source branch: `$(System.PullRequest.SourceBranch)`
    SourceBranch,
    /// PR target branch: `$(System.PullRequest.TargetBranch)`
    TargetBranch,
    /// Last commit message: `$(Build.SourceVersionMessage)`
    CommitMessage,
    /// Build reason: `$(Build.Reason)`
    BuildReason,
    /// Upstream pipeline name: `$(Build.TriggeredBy.DefinitionName)`
    TriggeredByPipeline,
    /// Triggering branch (non-PR): `$(Build.SourceBranch)`
    TriggeringBranch,

    // ── REST API-derived (requires API preamble) ────────────────────────
    /// Full PR metadata JSON from ADO REST API
    PrMetadata,
    /// PR draft status — extracted from PrMetadata
    PrIsDraft,
    /// PR labels list — extracted from PrMetadata
    PrLabels,

    // ── Iteration API-derived (separate API call) ───────────────────────
    /// List of changed file paths from PR iterations API
    ChangedFiles,
    /// Count of changed files (computed from ChangedFiles or fresh fetch)
    ChangedFileCount,

    // ── Computed at runtime ─────────────────────────────────────────────
    /// Current UTC time as minutes since midnight
    CurrentUtcMinutes,
}

impl Fact {
    /// Facts that must be acquired before this one.
    pub fn dependencies(&self) -> &'static [Fact] {
        match self {
            // Pipeline variables have no dependencies
            Fact::PrTitle
            | Fact::AuthorEmail
            | Fact::SourceBranch
            | Fact::TargetBranch
            | Fact::CommitMessage
            | Fact::BuildReason
            | Fact::TriggeredByPipeline
            | Fact::TriggeringBranch => &[],

            // API-derived facts
            Fact::PrMetadata => &[],
            Fact::PrIsDraft => &[Fact::PrMetadata],
            Fact::PrLabels => &[Fact::PrMetadata],

            // Iteration API
            Fact::ChangedFiles => &[],
            Fact::ChangedFileCount => &[Fact::ChangedFiles],

            // Computed
            Fact::CurrentUtcMinutes => &[],
        }
    }

    /// What to do if acquisition fails at runtime.
    pub fn failure_policy(&self) -> FailurePolicy {
        match self {
            // Pipeline variables are always available
            Fact::PrTitle
            | Fact::AuthorEmail
            | Fact::SourceBranch
            | Fact::TargetBranch
            | Fact::CommitMessage
            | Fact::BuildReason
            | Fact::TriggeredByPipeline
            | Fact::TriggeringBranch => FailurePolicy::FailClosed,

            // API failures: warn and skip dependent checks
            Fact::PrMetadata => FailurePolicy::SkipDependents,

            // Extraction failures from PR metadata
            Fact::PrIsDraft => FailurePolicy::FailClosed,
            Fact::PrLabels => FailurePolicy::FailOpen,

            // Changed files: fail open (assume match if can't determine)
            Fact::ChangedFiles => FailurePolicy::FailOpen,
            Fact::ChangedFileCount => FailurePolicy::FailOpen,

            // Time is always computable
            Fact::CurrentUtcMinutes => FailurePolicy::FailClosed,
        }
    }

    /// True if this fact is a free pipeline variable (no API/computation).
    pub fn is_pipeline_var(&self) -> bool {
        matches!(
            self,
            Fact::PrTitle
                | Fact::AuthorEmail
                | Fact::SourceBranch
                | Fact::TargetBranch
                | Fact::CommitMessage
                | Fact::BuildReason
                | Fact::TriggeredByPipeline
                | Fact::TriggeringBranch
        )
    }
}

/// What happens when a fact cannot be acquired at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailurePolicy {
    /// Check fails → SHOULD_RUN=false
    FailClosed,
    /// Check passes → assume OK
    FailOpen,
    /// Log warning, skip all predicates that depend on this fact
    SkipDependents,
}

// ─── Predicates ─────────────────────────────────────────────────────────────

/// A boolean test over one or more acquired facts.
#[derive(Debug, Clone)]
pub enum Predicate {
    /// Glob match: `fnmatch(value, pattern)` — `*` any chars, `?` single char
    GlobMatch { fact: Fact, pattern: String },

    /// Exact equality: `[ "$var" = "value" ]`
    Equality { fact: Fact, value: String },

    /// Value is in set (include): `echo "$var" | grep -qiE '^(a|b|c)$'`
    ValueInSet {
        fact: Fact,
        values: Vec<String>,
        case_insensitive: bool,
    },

    /// Value is NOT in set (exclude): inverse of ValueInSet
    ValueNotInSet {
        fact: Fact,
        values: Vec<String>,
        case_insensitive: bool,
    },

    /// Numeric range check: `[ "$var" -ge min ] && [ "$var" -le max ]`
    NumericRange {
        fact: Fact,
        min: Option<u32>,
        max: Option<u32>,
    },

    /// UTC time window check (handles overnight wrap).
    TimeWindow { start: String, end: String },

    /// Label set matching — typed collection predicate.
    /// Not flattened to space-separated string; codegen handles list semantics.
    LabelSetMatch {
        any_of: Vec<String>,
        all_of: Vec<String>,
        none_of: Vec<String>,
    },

    /// Changed file glob matching via python3 fnmatch.
    FileGlobMatch {
        include: Vec<String>,
        exclude: Vec<String>,
    },

    /// Logical AND — all must pass.
    And(Vec<Predicate>),
    /// Logical OR — at least one must pass.
    Or(Vec<Predicate>),
    /// Logical NOT — inner must fail.
    Not(Box<Predicate>),
}

impl Predicate {
    /// Collect all facts referenced by this predicate.
    pub fn required_facts(&self) -> BTreeSet<Fact> {
        let mut facts = BTreeSet::new();
        self.collect_facts(&mut facts);
        facts
    }

    fn collect_facts(&self, facts: &mut BTreeSet<Fact>) {
        match self {
            Predicate::GlobMatch { fact, .. }
            | Predicate::Equality { fact, .. }
            | Predicate::ValueInSet { fact, .. }
            | Predicate::ValueNotInSet { fact, .. }
            | Predicate::NumericRange { fact, .. } => {
                facts.insert(*fact);
            }
            Predicate::TimeWindow { .. } => {
                facts.insert(Fact::CurrentUtcMinutes);
            }
            Predicate::LabelSetMatch { .. } => {
                facts.insert(Fact::PrLabels);
            }
            Predicate::FileGlobMatch { .. } => {
                facts.insert(Fact::ChangedFiles);
            }
            Predicate::And(preds) | Predicate::Or(preds) => {
                for p in preds {
                    p.collect_facts(facts);
                }
            }
            Predicate::Not(inner) => {
                inner.collect_facts(facts);
            }
        }
    }
}

// ─── FilterCheck ────────────────────────────────────────────────────────────

/// A single filter check with metadata for diagnostics and bash codegen.
#[derive(Debug, Clone)]
pub struct FilterCheck {
    /// Human-readable name: "title", "author", "source-branch", etc.
    pub name: &'static str,
    /// The predicate to evaluate.
    pub predicate: Predicate,
    /// ADO build tag suffix on failure: e.g. "title-mismatch"
    pub build_tag_suffix: &'static str,
}

impl FilterCheck {
    /// All facts required by this check (including transitive dependencies).
    pub fn all_required_facts(&self) -> BTreeSet<Fact> {
        let direct = self.predicate.required_facts();
        let mut all = BTreeSet::new();
        for fact in &direct {
            collect_fact_with_deps(*fact, &mut all);
        }
        all
    }
}

/// Recursively collect a fact and all its transitive dependencies.
fn collect_fact_with_deps(fact: Fact, out: &mut BTreeSet<Fact>) {
    if out.insert(fact) {
        for dep in fact.dependencies() {
            collect_fact_with_deps(*dep, out);
        }
    }
}

// ─── Gate Context ───────────────────────────────────────────────────────────

/// Context for the gate step — determines bypass condition and tag prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateContext {
    /// PR trigger: bypass if `Build.Reason != PullRequest`
    PullRequest,
    /// Pipeline completion trigger: bypass if `Build.Reason != ResourceTrigger`
    PipelineCompletion,
}

impl GateContext {
    /// ADO Build.Reason value that activates this gate.
    pub fn build_reason(&self) -> &'static str {
        match self {
            GateContext::PullRequest => "PullRequest",
            GateContext::PipelineCompletion => "ResourceTrigger",
        }
    }

    /// Prefix for build tags emitted by this gate.
    pub fn tag_prefix(&self) -> &'static str {
        match self {
            GateContext::PullRequest => "pr-gate",
            GateContext::PipelineCompletion => "pipeline-gate",
        }
    }

    /// Display name for the gate step.
    pub fn display_name(&self) -> &'static str {
        match self {
            GateContext::PullRequest => "Evaluate PR filters",
            GateContext::PipelineCompletion => "Evaluate pipeline filters",
        }
    }

    /// Step name for the gate (used in output variable references).
    pub fn step_name(&self) -> &'static str {
        match self {
            GateContext::PullRequest => "prGate",
            GateContext::PipelineCompletion => "pipelineGate",
        }
    }
}

// ─── Diagnostics ────────────────────────────────────────────────────────────

/// Severity level for compile-time diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Informational — compilation continues.
    Info,
    /// Warning — compilation continues but user should review.
    Warning,
    /// Error — compilation fails.
    Error,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Severity::Info => write!(f, "info"),
            Severity::Warning => write!(f, "warning"),
            Severity::Error => write!(f, "error"),
        }
    }
}

/// A compile-time diagnostic about filter configuration.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    /// Severity level.
    pub severity: Severity,
    /// Which filter(s) this diagnostic concerns.
    pub filter: String,
    /// Human-readable message.
    pub message: String,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {} — {}", self.severity, self.filter, self.message)
    }
}

// ─── Lowering (Filters → IR) ───────────────────────────────────────────────

/// Lower `PrFilters` into a list of `FilterCheck` IR nodes.
pub fn lower_pr_filters(
    filters: &super::types::PrFilters,
) -> Vec<FilterCheck> {
    let mut checks = Vec::new();

    // Tier 1: Pipeline variables
    if let Some(title) = &filters.title {
        checks.push(FilterCheck {
            name: "title",
            predicate: Predicate::GlobMatch {
                fact: Fact::PrTitle,
                pattern: title.pattern.clone(),
            },
            build_tag_suffix: "title-mismatch",
        });
    }

    if let Some(author) = &filters.author {
        if !author.include.is_empty() {
            checks.push(FilterCheck {
                name: "author include",
                predicate: Predicate::ValueInSet {
                    fact: Fact::AuthorEmail,
                    values: author.include.clone(),
                    case_insensitive: true,
                },
                build_tag_suffix: "author-mismatch",
            });
        }
        if !author.exclude.is_empty() {
            checks.push(FilterCheck {
                name: "author exclude",
                predicate: Predicate::ValueNotInSet {
                    fact: Fact::AuthorEmail,
                    values: author.exclude.clone(),
                    case_insensitive: true,
                },
                build_tag_suffix: "author-excluded",
            });
        }
    }

    if let Some(source) = &filters.source_branch {
        checks.push(FilterCheck {
            name: "source-branch",
            predicate: Predicate::GlobMatch {
                fact: Fact::SourceBranch,
                pattern: source.pattern.clone(),
            },
            build_tag_suffix: "source-branch-mismatch",
        });
    }

    if let Some(target) = &filters.target_branch {
        checks.push(FilterCheck {
            name: "target-branch",
            predicate: Predicate::GlobMatch {
                fact: Fact::TargetBranch,
                pattern: target.pattern.clone(),
            },
            build_tag_suffix: "target-branch-mismatch",
        });
    }

    if let Some(cm) = &filters.commit_message {
        checks.push(FilterCheck {
            name: "commit-message",
            predicate: Predicate::GlobMatch {
                fact: Fact::CommitMessage,
                pattern: cm.pattern.clone(),
            },
            build_tag_suffix: "commit-message-mismatch",
        });
    }

    // Tier 2: REST API required
    if let Some(labels) = &filters.labels {
        checks.push(FilterCheck {
            name: "labels",
            predicate: Predicate::LabelSetMatch {
                any_of: labels.any_of.clone(),
                all_of: labels.all_of.clone(),
                none_of: labels.none_of.clone(),
            },
            build_tag_suffix: "labels-mismatch",
        });
    }

    if let Some(draft_expected) = filters.draft {
        checks.push(FilterCheck {
            name: "draft",
            predicate: Predicate::Equality {
                fact: Fact::PrIsDraft,
                value: if draft_expected {
                    "true".into()
                } else {
                    "false".into()
                },
            },
            build_tag_suffix: "draft-mismatch",
        });
    }

    if let Some(cf) = &filters.changed_files {
        checks.push(FilterCheck {
            name: "changed-files",
            predicate: Predicate::FileGlobMatch {
                include: cf.include.clone(),
                exclude: cf.exclude.clone(),
            },
            build_tag_suffix: "changed-files-mismatch",
        });
    }

    // Tier 3: Advanced
    if let Some(tw) = &filters.time_window {
        checks.push(FilterCheck {
            name: "time-window",
            predicate: Predicate::TimeWindow {
                start: tw.start.clone(),
                end: tw.end.clone(),
            },
            build_tag_suffix: "time-window-mismatch",
        });
    }

    if filters.min_changes.is_some() || filters.max_changes.is_some() {
        checks.push(FilterCheck {
            name: "change-count",
            predicate: Predicate::NumericRange {
                fact: Fact::ChangedFileCount,
                min: filters.min_changes,
                max: filters.max_changes,
            },
            build_tag_suffix: "changes-mismatch",
        });
    }

    if let Some(br) = &filters.build_reason {
        if !br.include.is_empty() {
            checks.push(FilterCheck {
                name: "build-reason include",
                predicate: Predicate::ValueInSet {
                    fact: Fact::BuildReason,
                    values: br.include.clone(),
                    case_insensitive: true,
                },
                build_tag_suffix: "build-reason-mismatch",
            });
        }
        if !br.exclude.is_empty() {
            checks.push(FilterCheck {
                name: "build-reason exclude",
                predicate: Predicate::ValueNotInSet {
                    fact: Fact::BuildReason,
                    values: br.exclude.clone(),
                    case_insensitive: true,
                },
                build_tag_suffix: "build-reason-excluded",
            });
        }
    }

    checks
}

/// Lower `PipelineFilters` into a list of `FilterCheck` IR nodes.
pub fn lower_pipeline_filters(
    filters: &super::types::PipelineFilters,
) -> Vec<FilterCheck> {
    let mut checks = Vec::new();

    if let Some(sp) = &filters.source_pipeline {
        checks.push(FilterCheck {
            name: "source-pipeline",
            predicate: Predicate::GlobMatch {
                fact: Fact::TriggeredByPipeline,
                pattern: sp.pattern.clone(),
            },
            build_tag_suffix: "source-pipeline-mismatch",
        });
    }

    if let Some(branch) = &filters.branch {
        checks.push(FilterCheck {
            name: "branch",
            predicate: Predicate::GlobMatch {
                fact: Fact::TriggeringBranch,
                pattern: branch.pattern.clone(),
            },
            build_tag_suffix: "branch-mismatch",
        });
    }

    if let Some(tw) = &filters.time_window {
        checks.push(FilterCheck {
            name: "time-window",
            predicate: Predicate::TimeWindow {
                start: tw.start.clone(),
                end: tw.end.clone(),
            },
            build_tag_suffix: "time-window-mismatch",
        });
    }

    if let Some(br) = &filters.build_reason {
        if !br.include.is_empty() {
            checks.push(FilterCheck {
                name: "build-reason include",
                predicate: Predicate::ValueInSet {
                    fact: Fact::BuildReason,
                    values: br.include.clone(),
                    case_insensitive: true,
                },
                build_tag_suffix: "build-reason-mismatch",
            });
        }
        if !br.exclude.is_empty() {
            checks.push(FilterCheck {
                name: "build-reason exclude",
                predicate: Predicate::ValueNotInSet {
                    fact: Fact::BuildReason,
                    values: br.exclude.clone(),
                    case_insensitive: true,
                },
                build_tag_suffix: "build-reason-excluded",
            });
        }
    }

    checks
}

// ─── Validation ─────────────────────────────────────────────────────────────

/// Validate filter configuration for conflicts and impossible combinations.
///
/// Checks are performed on the original filter structs (not just the IR)
/// because some validations need field-level context.
pub fn validate_pr_filters(filters: &super::types::PrFilters) -> Vec<Diagnostic> {
    let mut diags = Vec::new();

    // min_changes > max_changes
    if let (Some(min), Some(max)) = (filters.min_changes, filters.max_changes) {
        if min > max {
            diags.push(Diagnostic {
                severity: Severity::Error,
                filter: "min-changes / max-changes".into(),
                message: format!(
                    "min-changes ({}) is greater than max-changes ({}) — no PR can satisfy both",
                    min, max
                ),
            });
        }
    }

    // Time window validation
    if let Some(tw) = &filters.time_window {
        if !is_valid_time(tw.start.as_str()) {
            diags.push(Diagnostic {
                severity: Severity::Error,
                filter: "time-window".into(),
                message: format!(
                    "start '{}' is not valid HH:MM format",
                    tw.start
                ),
            });
        }
        if !is_valid_time(tw.end.as_str()) {
            diags.push(Diagnostic {
                severity: Severity::Error,
                filter: "time-window".into(),
                message: format!(
                    "end '{}' is not valid HH:MM format",
                    tw.end
                ),
            });
        }
        if tw.start == tw.end {
            diags.push(Diagnostic {
                severity: Severity::Error,
                filter: "time-window".into(),
                message: format!(
                    "start ({}) equals end ({}) — this is a zero-width window that never matches",
                    tw.start, tw.end
                ),
            });
        }
    }

    // Author include/exclude overlap
    if let Some(author) = &filters.author {
        let overlap = find_overlap(&author.include, &author.exclude);
        if !overlap.is_empty() {
            diags.push(Diagnostic {
                severity: Severity::Error,
                filter: "author".into(),
                message: format!(
                    "values appear in both include and exclude lists: {}",
                    overlap.join(", ")
                ),
            });
        }
    }

    // Build reason include/exclude overlap
    if let Some(br) = &filters.build_reason {
        let overlap = find_overlap(&br.include, &br.exclude);
        if !overlap.is_empty() {
            diags.push(Diagnostic {
                severity: Severity::Error,
                filter: "build-reason".into(),
                message: format!(
                    "values appear in both include and exclude lists: {}",
                    overlap.join(", ")
                ),
            });
        }
    }

    // Labels conflicts
    if let Some(labels) = &filters.labels {
        // any-of ∩ none-of
        let overlap = find_overlap(&labels.any_of, &labels.none_of);
        if !overlap.is_empty() {
            diags.push(Diagnostic {
                severity: Severity::Error,
                filter: "labels".into(),
                message: format!(
                    "labels appear in both any-of and none-of: {}",
                    overlap.join(", ")
                ),
            });
        }
        // all-of ∩ none-of
        let overlap = find_overlap(&labels.all_of, &labels.none_of);
        if !overlap.is_empty() {
            diags.push(Diagnostic {
                severity: Severity::Error,
                filter: "labels".into(),
                message: format!(
                    "labels appear in both all-of and none-of: {}",
                    overlap.join(", ")
                ),
            });
        }
        // Empty any-of/all-of with no none-of (likely mistake)
        if labels.any_of.is_empty() && labels.all_of.is_empty() && labels.none_of.is_empty() {
            diags.push(Diagnostic {
                severity: Severity::Warning,
                filter: "labels".into(),
                message: "labels filter is empty — no label checks will be applied".into(),
            });
        }
    }

    diags
}

/// Validate pipeline filter configuration for conflicts.
pub fn validate_pipeline_filters(
    filters: &super::types::PipelineFilters,
) -> Vec<Diagnostic> {
    let mut diags = Vec::new();

    if let Some(tw) = &filters.time_window {
        if !is_valid_time(tw.start.as_str()) {
            diags.push(Diagnostic {
                severity: Severity::Error,
                filter: "time-window".into(),
                message: format!("start '{}' is not valid HH:MM format", tw.start),
            });
        }
        if !is_valid_time(tw.end.as_str()) {
            diags.push(Diagnostic {
                severity: Severity::Error,
                filter: "time-window".into(),
                message: format!("end '{}' is not valid HH:MM format", tw.end),
            });
        }
        if tw.start == tw.end {
            diags.push(Diagnostic {
                severity: Severity::Error,
                filter: "time-window".into(),
                message: format!(
                    "start ({}) equals end ({}) — this is a zero-width window that never matches",
                    tw.start, tw.end
                ),
            });
        }
    }

    if let Some(br) = &filters.build_reason {
        let overlap = find_overlap(&br.include, &br.exclude);
        if !overlap.is_empty() {
            diags.push(Diagnostic {
                severity: Severity::Error,
                filter: "build-reason".into(),
                message: format!(
                    "values appear in both include and exclude lists: {}",
                    overlap.join(", ")
                ),
            });
        }
    }

    diags
}

/// Find case-insensitive overlap between two string slices.
fn find_overlap(a: &[String], b: &[String]) -> Vec<String> {
    let a_lower: BTreeSet<String> = a.iter().map(|s| s.to_lowercase()).collect();
    let b_lower: BTreeSet<String> = b.iter().map(|s| s.to_lowercase()).collect();
    a_lower.intersection(&b_lower).cloned().collect()
}

/// Validate that a string is in HH:MM format (00:00–23:59).
fn is_valid_time(s: &str) -> bool {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 2 {
        return false;
    }
    let Ok(h) = parts[0].parse::<u32>() else {
        return false;
    };
    let Ok(m) = parts[1].parse::<u32>() else {
        return false;
    };
    h < 24 && m < 60
}

// ─── Serializable Gate Spec ─────────────────────────────────────────────────

use serde::Serialize;
use schemars::JsonSchema;

/// Serializable gate specification — the JSON document consumed by the
/// Python gate evaluator at pipeline runtime.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct GateSpec {
    pub context: GateContextSpec,
    pub facts: Vec<FactSpec>,
    pub checks: Vec<CheckSpec>,
}

/// Serialized gate context.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct GateContextSpec {
    pub build_reason: String,
    pub tag_prefix: String,
    pub step_name: String,
    pub bypass_label: String,
}

/// Serialized fact acquisition descriptor.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct FactSpec {
    pub kind: String,
    pub failure_policy: String,
}

/// Serialized filter check.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CheckSpec {
    pub name: String,
    pub predicate: PredicateSpec,
    pub tag_suffix: String,
}

/// Serialized predicate — the expression tree evaluated at runtime.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(tag = "type")]
pub enum PredicateSpec {
    #[serde(rename = "glob_match")]
    GlobMatch { fact: String, pattern: String },

    #[serde(rename = "equals")]
    Equals { fact: String, value: String },

    #[serde(rename = "value_in_set")]
    ValueInSet {
        fact: String,
        values: Vec<String>,
        case_insensitive: bool,
    },

    #[serde(rename = "value_not_in_set")]
    ValueNotInSet {
        fact: String,
        values: Vec<String>,
        case_insensitive: bool,
    },

    #[serde(rename = "numeric_range")]
    NumericRange {
        fact: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        min: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        max: Option<u32>,
    },

    #[serde(rename = "time_window")]
    TimeWindow { start: String, end: String },

    #[serde(rename = "label_set_match")]
    LabelSetMatch {
        fact: String,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        any_of: Vec<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        all_of: Vec<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        none_of: Vec<String>,
    },

    #[serde(rename = "file_glob_match")]
    FileGlobMatch {
        fact: String,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        include: Vec<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        exclude: Vec<String>,
    },

    #[serde(rename = "and")]
    And { operands: Vec<PredicateSpec> },

    #[serde(rename = "or")]
    Or { operands: Vec<PredicateSpec> },

    #[serde(rename = "not")]
    Not { operand: Box<PredicateSpec> },
}

/// Generate the JSON Schema for the gate spec.
///
/// This schema is the formal contract between the Rust compiler and the
/// Python evaluator. It should be shipped in `scripts/gate-spec.schema.json`
/// alongside the evaluator.
pub fn generate_gate_spec_schema() -> String {
    let schema = schemars::schema_for!(GateSpec);
    serde_json::to_string_pretty(&schema).expect("schema serialization")
}

// ─── Codegen ────────────────────────────────────────────────────────────────

// The inline heredoc evaluator has been removed in favor of external script delivery.
// See TriggerFiltersExtension for the external path and compile_gate_step_inline for Tier 1.

impl Fact {
    /// ADO macro exports required by this fact.
    ///
    /// Returns `(env_var_name, ado_macro)` pairs that must be exported in
    /// the bash shim for the Python evaluator to read.
    pub fn ado_exports(&self) -> Vec<(&'static str, &'static str)> {
        match self {
            Fact::PrTitle => vec![("ADO_PR_TITLE", "$(System.PullRequest.Title)")],
            Fact::AuthorEmail => vec![("ADO_AUTHOR_EMAIL", "$(Build.RequestedForEmail)")],
            Fact::SourceBranch => {
                vec![("ADO_SOURCE_BRANCH", "$(System.PullRequest.SourceBranch)")]
            }
            Fact::TargetBranch => {
                vec![("ADO_TARGET_BRANCH", "$(System.PullRequest.TargetBranch)")]
            }
            Fact::CommitMessage => {
                vec![("ADO_COMMIT_MESSAGE", "$(Build.SourceVersionMessage)")]
            }
            Fact::BuildReason => vec![("ADO_BUILD_REASON", "$(Build.Reason)")],
            Fact::TriggeredByPipeline => vec![(
                "ADO_TRIGGERED_BY_PIPELINE",
                "$(Build.TriggeredBy.DefinitionName)",
            )],
            Fact::TriggeringBranch => {
                vec![("ADO_TRIGGERING_BRANCH", "$(Build.SourceBranch)")]
            }
            // API-derived and computed facts don't need ADO macro exports —
            // the evaluator handles acquisition internally.
            Fact::PrMetadata | Fact::PrIsDraft | Fact::PrLabels => vec![],
            Fact::ChangedFiles | Fact::ChangedFileCount => vec![],
            Fact::CurrentUtcMinutes => vec![],
        }
    }

    /// The fact kind string used in the serialized spec.
    pub fn kind(&self) -> &'static str {
        match self {
            Fact::PrTitle => "pr_title",
            Fact::AuthorEmail => "author_email",
            Fact::SourceBranch => "source_branch",
            Fact::TargetBranch => "target_branch",
            Fact::CommitMessage => "commit_message",
            Fact::BuildReason => "build_reason",
            Fact::TriggeredByPipeline => "triggered_by_pipeline",
            Fact::TriggeringBranch => "triggering_branch",
            Fact::PrMetadata => "pr_metadata",
            Fact::PrIsDraft => "pr_is_draft",
            Fact::PrLabels => "pr_labels",
            Fact::ChangedFiles => "changed_files",
            Fact::ChangedFileCount => "changed_file_count",
            Fact::CurrentUtcMinutes => "current_utc_minutes",
        }
    }
}

impl FailurePolicy {
    fn as_str(&self) -> &'static str {
        match self {
            FailurePolicy::FailClosed => "fail_closed",
            FailurePolicy::FailOpen => "fail_open",
            FailurePolicy::SkipDependents => "skip_dependents",
        }
    }
}

/// Convert a `Predicate` to its serializable spec form.
fn predicate_to_spec(pred: &Predicate) -> PredicateSpec {
    match pred {
        Predicate::GlobMatch { fact, pattern } => PredicateSpec::GlobMatch {
            fact: fact.kind().into(),
            pattern: pattern.clone(),
        },
        Predicate::Equality { fact, value } => PredicateSpec::Equals {
            fact: fact.kind().into(),
            value: value.clone(),
        },
        Predicate::ValueInSet {
            fact,
            values,
            case_insensitive,
        } => PredicateSpec::ValueInSet {
            fact: fact.kind().into(),
            values: values.clone(),
            case_insensitive: *case_insensitive,
        },
        Predicate::ValueNotInSet {
            fact,
            values,
            case_insensitive,
        } => PredicateSpec::ValueNotInSet {
            fact: fact.kind().into(),
            values: values.clone(),
            case_insensitive: *case_insensitive,
        },
        Predicate::NumericRange { fact, min, max } => PredicateSpec::NumericRange {
            fact: fact.kind().into(),
            min: *min,
            max: *max,
        },
        Predicate::TimeWindow { start, end } => PredicateSpec::TimeWindow {
            start: start.clone(),
            end: end.clone(),
        },
        Predicate::LabelSetMatch {
            any_of,
            all_of,
            none_of,
        } => PredicateSpec::LabelSetMatch {
            fact: Fact::PrLabels.kind().into(),
            any_of: any_of.clone(),
            all_of: all_of.clone(),
            none_of: none_of.clone(),
        },
        Predicate::FileGlobMatch { include, exclude } => PredicateSpec::FileGlobMatch {
            fact: Fact::ChangedFiles.kind().into(),
            include: include.clone(),
            exclude: exclude.clone(),
        },
        Predicate::And(preds) => PredicateSpec::And {
            operands: preds.iter().map(predicate_to_spec).collect(),
        },
        Predicate::Or(preds) => PredicateSpec::Or {
            operands: preds.iter().map(predicate_to_spec).collect(),
        },
        Predicate::Not(inner) => PredicateSpec::Not {
            operand: Box::new(predicate_to_spec(inner)),
        },
    }
}

/// Build a `GateSpec` from a gate context and filter checks.
pub fn build_gate_spec(ctx: GateContext, checks: &[FilterCheck]) -> GateSpec {
    let facts_set = collect_ordered_facts(checks);

    let facts: Vec<FactSpec> = facts_set
        .iter()
        .map(|f| FactSpec {
            kind: f.kind().into(),
            failure_policy: f.failure_policy().as_str().into(),
        })
        .collect();

    let spec_checks: Vec<CheckSpec> = checks
        .iter()
        .map(|c| CheckSpec {
            name: c.name.into(),
            predicate: predicate_to_spec(&c.predicate),
            tag_suffix: c.build_tag_suffix.into(),
        })
        .collect();

    GateSpec {
        context: GateContextSpec {
            build_reason: ctx.build_reason().into(),
            tag_prefix: ctx.tag_prefix().into(),
            step_name: ctx.step_name().into(),
            bypass_label: match ctx {
                GateContext::PullRequest => "PR",
                GateContext::PipelineCompletion => "pipeline",
            }
            .into(),
        },
        facts,
        checks: spec_checks,
    }
}

/// Compile filter checks into a bash gate step using an external evaluator
/// script. ADO variables are passed via the step's `env:` block (idiomatic
/// ADO pattern), and the gate spec is base64-encoded in GATE_SPEC.
pub fn compile_gate_step_external(
    ctx: GateContext,
    checks: &[FilterCheck],
    evaluator_path: &str,
) -> String {
    use base64::{engine::general_purpose::STANDARD, Engine as _};

    if checks.is_empty() {
        return String::new();
    }

    let spec = build_gate_spec(ctx, checks);
    let spec_json = serde_json::to_string(&spec).expect("gate spec serialization");
    let spec_b64 = STANDARD.encode(spec_json.as_bytes());

    let exports = collect_ado_exports(checks);

    let mut step = String::new();
    step.push_str(&format!("- bash: python3 {}\n", evaluator_path));
    step.push_str(&format!("  name: {}\n", ctx.step_name()));
    step.push_str(&format!(
        "  displayName: \"{}\"\n",
        ctx.display_name()
    ));
    step.push_str("  env:\n");
    step.push_str("    SYSTEM_ACCESSTOKEN: $(System.AccessToken)\n");
    step.push_str(&format!("    GATE_SPEC: \"{}\"\n", spec_b64));

    for (env_var, ado_macro) in &exports {
        step.push_str(&format!("    {}: {}\n", env_var, ado_macro));
    }

    step
}



/// Collect ADO macro exports needed by the given checks.
fn collect_ado_exports(checks: &[FilterCheck]) -> Vec<(&'static str, &'static str)> {
    let facts_set = collect_ordered_facts(checks);
    let mut exports: Vec<(&str, &str)> = Vec::new();
    let mut seen = BTreeSet::new();

    // Always-needed infra vars
    let infra: Vec<(&str, &str)> = vec![
        ("ADO_BUILD_REASON", "$(Build.Reason)"),
        ("ADO_COLLECTION_URI", "$(System.CollectionUri)"),
        ("ADO_PROJECT", "$(System.TeamProject)"),
        ("ADO_BUILD_ID", "$(Build.BuildId)"),
    ];
    for (k, v) in &infra {
        if seen.insert(*k) {
            exports.push((k, v));
        }
    }

    let needs_pr_api = facts_set.iter().any(|f| {
        matches!(
            f,
            Fact::PrMetadata | Fact::PrIsDraft | Fact::PrLabels | Fact::ChangedFiles
        )
    });
    if needs_pr_api {
        if seen.insert("ADO_REPO_ID") {
            exports.push(("ADO_REPO_ID", "$(Build.Repository.ID)"));
        }
        if seen.insert("ADO_PR_ID") {
            exports.push(("ADO_PR_ID", "$(System.PullRequest.PullRequestId)"));
        }
    }

    for fact in &facts_set {
        for (env_var, ado_macro) in fact.ado_exports() {
            if seen.insert(env_var) {
                exports.push((env_var, ado_macro));
            }
        }
    }
    exports
}


/// Collect all facts required by checks, topo-sorted by dependencies.
fn collect_ordered_facts(checks: &[FilterCheck]) -> Vec<Fact> {
    let mut all_facts = BTreeSet::new();
    for check in checks {
        for fact in check.all_required_facts() {
            all_facts.insert(fact);
        }
    }
    all_facts.into_iter().collect()
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::types::*;

    // ─── Fact tests ─────────────────────────────────────────────────────

    #[test]
    fn test_pipeline_var_facts_have_no_dependencies() {
        let pipeline_facts = [
            Fact::PrTitle,
            Fact::AuthorEmail,
            Fact::SourceBranch,
            Fact::TargetBranch,
            Fact::CommitMessage,
            Fact::BuildReason,
        ];
        for fact in &pipeline_facts {
            assert!(
                fact.dependencies().is_empty(),
                "{:?} should have no dependencies",
                fact
            );
            assert!(
                fact.is_pipeline_var(),
                "{:?} should be a pipeline var",
                fact
            );
        }
    }

    #[test]
    fn test_api_derived_facts_have_dependencies() {
        assert_eq!(Fact::PrIsDraft.dependencies(), &[Fact::PrMetadata]);
        assert_eq!(Fact::PrLabels.dependencies(), &[Fact::PrMetadata]);
    }

    #[test]
    fn test_fact_kinds_are_unique() {
        let all_facts = [
            Fact::PrTitle,
            Fact::AuthorEmail,
            Fact::SourceBranch,
            Fact::TargetBranch,
            Fact::CommitMessage,
            Fact::BuildReason,
            Fact::TriggeredByPipeline,
            Fact::TriggeringBranch,
            Fact::PrMetadata,
            Fact::PrIsDraft,
            Fact::PrLabels,
            Fact::ChangedFiles,
            Fact::ChangedFileCount,
            Fact::CurrentUtcMinutes,
        ];
        let kinds: BTreeSet<&str> =
            all_facts.iter().map(|f| f.kind()).collect();
        assert_eq!(kinds.len(), all_facts.len(), "fact kind strings must be unique");
    }

    // ─── Lowering tests ────────────────────────────────────────────────

    #[test]
    fn test_lower_pr_filters_empty() {
        let filters = PrFilters::default();
        let checks = lower_pr_filters(&filters);
        assert!(checks.is_empty());
    }

    #[test]
    fn test_lower_pr_filters_title() {
        let filters = PrFilters {
            title: Some(PatternFilter {
                pattern: "*[review]*".into(),
            }),
            ..Default::default()
        };
        let checks = lower_pr_filters(&filters);
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].name, "title");
        assert!(matches!(
            &checks[0].predicate,
            Predicate::GlobMatch { fact: Fact::PrTitle, pattern } if pattern == "*[review]*"
        ));
    }

    #[test]
    fn test_lower_pr_filters_author_include_exclude() {
        let filters = PrFilters {
            author: Some(IncludeExcludeFilter {
                include: vec!["alice@corp.com".into()],
                exclude: vec!["bot@noreply.com".into()],
            }),
            ..Default::default()
        };
        let checks = lower_pr_filters(&filters);
        assert_eq!(checks.len(), 2);
        assert_eq!(checks[0].name, "author include");
        assert_eq!(checks[1].name, "author exclude");
    }

    #[test]
    fn test_lower_pr_filters_labels() {
        let filters = PrFilters {
            labels: Some(LabelFilter {
                any_of: vec!["run-agent".into()],
                all_of: vec![],
                none_of: vec!["do-not-run".into()],
            }),
            ..Default::default()
        };
        let checks = lower_pr_filters(&filters);
        assert_eq!(checks.len(), 1);
        assert!(matches!(&checks[0].predicate, Predicate::LabelSetMatch { .. }));
    }

    #[test]
    fn test_lower_pr_filters_change_count() {
        let filters = PrFilters {
            min_changes: Some(5),
            max_changes: Some(100),
            ..Default::default()
        };
        let checks = lower_pr_filters(&filters);
        assert_eq!(checks.len(), 1);
        assert!(matches!(
            &checks[0].predicate,
            Predicate::NumericRange { min: Some(5), max: Some(100), .. }
        ));
    }

    #[test]
    fn test_lower_pipeline_filters() {
        let filters = PipelineFilters {
            source_pipeline: Some(PatternFilter {
                pattern: "Build.*".into(),
            }),
            branch: Some(PatternFilter {
                pattern: "^refs/heads/main$".into(),
            }),
            time_window: None,
            build_reason: None,
            expression: None,
        };
        let checks = lower_pipeline_filters(&filters);
        assert_eq!(checks.len(), 2);
        assert_eq!(checks[0].name, "source-pipeline");
        assert_eq!(checks[1].name, "branch");
    }

    // ─── Validation tests ──────────────────────────────────────────────

    #[test]
    fn test_validate_min_greater_than_max() {
        let filters = PrFilters {
            min_changes: Some(100),
            max_changes: Some(5),
            ..Default::default()
        };
        let diags = validate_pr_filters(&filters);
        assert!(diags.iter().any(|d| d.severity == Severity::Error
            && d.filter.contains("min-changes")));
    }

    #[test]
    fn test_validate_time_window_zero_width() {
        let filters = PrFilters {
            time_window: Some(TimeWindowFilter {
                start: "09:00".into(),
                end: "09:00".into(),
            }),
            ..Default::default()
        };
        let diags = validate_pr_filters(&filters);
        assert!(diags
            .iter()
            .any(|d| d.severity == Severity::Error && d.filter == "time-window"));
    }

    #[test]
    fn test_validate_author_overlap() {
        let filters = PrFilters {
            author: Some(IncludeExcludeFilter {
                include: vec!["alice@corp.com".into()],
                exclude: vec!["alice@corp.com".into()],
            }),
            ..Default::default()
        };
        let diags = validate_pr_filters(&filters);
        assert!(diags
            .iter()
            .any(|d| d.severity == Severity::Error && d.filter == "author"));
    }

    #[test]
    fn test_validate_label_any_of_none_of_conflict() {
        let filters = PrFilters {
            labels: Some(LabelFilter {
                any_of: vec!["run-agent".into()],
                all_of: vec![],
                none_of: vec!["run-agent".into()],
            }),
            ..Default::default()
        };
        let diags = validate_pr_filters(&filters);
        assert!(diags
            .iter()
            .any(|d| d.severity == Severity::Error && d.filter == "labels"));
    }

    #[test]
    fn test_validate_label_all_of_none_of_conflict() {
        let filters = PrFilters {
            labels: Some(LabelFilter {
                any_of: vec![],
                all_of: vec!["important".into()],
                none_of: vec!["important".into()],
            }),
            ..Default::default()
        };
        let diags = validate_pr_filters(&filters);
        assert!(diags
            .iter()
            .any(|d| d.severity == Severity::Error && d.filter == "labels"));
    }

    #[test]
    fn test_validate_build_reason_overlap() {
        let filters = PrFilters {
            build_reason: Some(IncludeExcludeFilter {
                include: vec!["PullRequest".into()],
                exclude: vec!["PullRequest".into()],
            }),
            ..Default::default()
        };
        let diags = validate_pr_filters(&filters);
        assert!(diags
            .iter()
            .any(|d| d.severity == Severity::Error && d.filter == "build-reason"));
    }

    #[test]
    fn test_validate_no_errors_for_valid_filters() {
        let filters = PrFilters {
            title: Some(PatternFilter {
                pattern: "*[review]*".into(),
            }),
            min_changes: Some(1),
            max_changes: Some(50),
            time_window: Some(TimeWindowFilter {
                start: "09:00".into(),
                end: "17:00".into(),
            }),
            ..Default::default()
        };
        let diags = validate_pr_filters(&filters);
        assert!(
            diags.iter().all(|d| d.severity != Severity::Error),
            "valid filters should produce no errors: {:?}",
            diags
        );
    }

    // ─── Codegen tests ─────────────────────────────────────────────────

    #[test]
    fn test_compile_gate_step_empty() {
        let result = compile_gate_step_external(GateContext::PullRequest, &[], "/tmp/ado-aw-scripts/gate-eval.py");
        assert!(result.is_empty());
    }

    #[test]
    fn test_compile_gate_step_structure() {
        let checks = vec![FilterCheck {
            name: "title",
            predicate: Predicate::GlobMatch {
                fact: Fact::PrTitle,
                pattern: "test".into(),
            },
            build_tag_suffix: "title-mismatch",
        }];
        let result = compile_gate_step_external(GateContext::PullRequest, &checks, "/tmp/ado-aw-scripts/gate-eval.py");
        assert!(result.contains("- bash:"), "should be a bash step");
        assert!(result.contains("GATE_SPEC"), "should include base64 spec in env");
        assert!(result.contains("python3 /tmp/ado-aw-scripts/gate-eval.py"), "should reference external evaluator script");
        assert!(result.contains("name: prGate"), "should set step name");
        assert!(result.contains("SYSTEM_ACCESSTOKEN"), "should pass access token via env block");
    }

    #[test]
    fn test_compile_gate_step_exports_ado_macros() {
        let checks = vec![FilterCheck {
            name: "title",
            predicate: Predicate::GlobMatch {
                fact: Fact::PrTitle,
                pattern: "test".into(),
            },
            build_tag_suffix: "title-mismatch",
        }];
        let result = compile_gate_step_external(GateContext::PullRequest, &checks, "/tmp/ado-aw-scripts/gate-eval.py");
        assert!(result.contains("ADO_BUILD_REASON"), "should export build reason");
        assert!(result.contains("ADO_PR_TITLE"), "should export PR title");
        assert!(result.contains("$(System.PullRequest.Title)"), "should reference ADO macro");
    }

    #[test]
    fn test_compile_gate_step_pipeline_context() {
        let checks = vec![FilterCheck {
            name: "source-pipeline",
            predicate: Predicate::GlobMatch {
                fact: Fact::TriggeredByPipeline,
                pattern: "Build.*".into(),
            },
            build_tag_suffix: "source-pipeline-mismatch",
        }];
        let result = compile_gate_step_external(GateContext::PipelineCompletion, &checks, "/tmp/ado-aw-scripts/gate-eval.py");
        assert!(result.contains("name: pipelineGate"), "should set pipeline gate name");
        assert!(result.contains("Evaluate pipeline filters"), "should set display name");
        assert!(result.contains("ADO_TRIGGERED_BY_PIPELINE"), "should export pipeline macro");
    }

    #[test]
    fn test_compile_gate_step_exports_pr_api_vars_for_tier2() {
        let checks = vec![FilterCheck {
            name: "draft",
            predicate: Predicate::Equality {
                fact: Fact::PrIsDraft,
                value: "false".into(),
            },
            build_tag_suffix: "draft-mismatch",
        }];
        let result = compile_gate_step_external(GateContext::PullRequest, &checks, "/tmp/ado-aw-scripts/gate-eval.py");
        assert!(result.contains("ADO_REPO_ID"), "should export repo ID for API calls");
        assert!(result.contains("ADO_PR_ID"), "should export PR ID for API calls");
    }

    #[test]
    fn test_compile_gate_step_no_pr_api_vars_for_tier1() {
        let checks = vec![FilterCheck {
            name: "title",
            predicate: Predicate::GlobMatch {
                fact: Fact::PrTitle,
                pattern: "test".into(),
            },
            build_tag_suffix: "title-mismatch",
        }];
        let result = compile_gate_step_external(GateContext::PullRequest, &checks, "/tmp/ado-aw-scripts/gate-eval.py");
        // Check export lines only (evaluator script always contains these strings)
        assert!(!result.contains("ADO_REPO_ID:"), "should not export repo ID for title-only");
        assert!(!result.contains("ADO_PR_ID:"), "should not export PR ID for title-only");
    }

    #[test]
    fn test_build_gate_spec_structure() {
        let checks = vec![
            FilterCheck {
                name: "title",
                predicate: Predicate::GlobMatch {
                    fact: Fact::PrTitle,
                    pattern: "test".into(),
                },
                build_tag_suffix: "title-mismatch",
            },
            FilterCheck {
                name: "labels",
                predicate: Predicate::LabelSetMatch {
                    any_of: vec!["run-agent".into()],
                    all_of: vec![],
                    none_of: vec!["do-not-run".into()],
                },
                build_tag_suffix: "labels-mismatch",
            },
        ];
        let spec = build_gate_spec(GateContext::PullRequest, &checks);
        assert_eq!(spec.context.build_reason, "PullRequest");
        assert_eq!(spec.context.tag_prefix, "pr-gate");
        assert_eq!(spec.context.step_name, "prGate");
        assert_eq!(spec.context.bypass_label, "PR");
        // Facts should include pr_title, pr_metadata (dep of pr_labels), pr_labels
        assert!(spec.facts.iter().any(|f| f.kind == "pr_title"));
        assert!(spec.facts.iter().any(|f| f.kind == "pr_metadata"));
        assert!(spec.facts.iter().any(|f| f.kind == "pr_labels"));
        // Checks
        assert_eq!(spec.checks.len(), 2);
        assert_eq!(spec.checks[0].name, "title");
        assert_eq!(spec.checks[1].name, "labels");
    }

    #[test]
    fn test_gate_spec_serializes_to_valid_json() {
        let checks = vec![FilterCheck {
            name: "title",
            predicate: Predicate::GlobMatch {
                fact: Fact::PrTitle,
                pattern: "*[review]*".into(),
            },
            build_tag_suffix: "title-mismatch",
        }];
        let spec = build_gate_spec(GateContext::PullRequest, &checks);
        let json = serde_json::to_string(&spec).unwrap();
        // Should roundtrip
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["context"]["build_reason"], "PullRequest");
        assert_eq!(parsed["checks"][0]["name"], "title");
        assert_eq!(parsed["checks"][0]["predicate"]["type"], "glob_match");
        assert_eq!(parsed["checks"][0]["predicate"]["pattern"], "*[review]*");
    }

    // ─── End-to-end lowering + codegen ──────────────────────────────────

    #[test]
    fn test_roundtrip_pr_filters_to_gate_step() {
        let filters = PrFilters {
            title: Some(PatternFilter {
                pattern: "*[review]*".into(),
            }),
            draft: Some(false),
            labels: Some(LabelFilter {
                any_of: vec!["run-agent".into()],
                all_of: vec![],
                none_of: vec![],
            }),
            ..Default::default()
        };
        let checks = lower_pr_filters(&filters);
        let diags = validate_pr_filters(&filters);
        assert!(diags.iter().all(|d| d.severity != Severity::Error));

        let step = compile_gate_step_external(GateContext::PullRequest, &checks, "/tmp/ado-aw-scripts/gate-eval.py");
        // Step structure
        assert!(step.contains("ADO_PR_TITLE"));
        assert!(step.contains("ADO_REPO_ID")); // for API-derived facts
        assert!(step.contains("python3"));
        assert!(step.contains("prGate"));

        // Spec content
        let spec = build_gate_spec(GateContext::PullRequest, &checks);
        assert_eq!(spec.checks.len(), 3);
        assert!(spec.facts.iter().any(|f| f.kind == "pr_title"));
        assert!(spec.facts.iter().any(|f| f.kind == "pr_is_draft"));
        assert!(spec.facts.iter().any(|f| f.kind == "pr_labels"));
    }

    // ─── Schema tests ──────────────────────────────────────────────────

    #[test]
    fn test_generate_schema_is_valid_json() {
        let schema = generate_gate_spec_schema();
        let parsed: serde_json::Value = serde_json::from_str(&schema)
            .expect("schema should be valid JSON");
        assert!(parsed.is_object());
        assert!(parsed.get("$schema").is_some() || parsed.get("type").is_some(),
            "should be a JSON Schema document");
    }

    #[test]
    fn test_schema_includes_all_predicate_types() {
        let schema = generate_gate_spec_schema();
        // All predicate type discriminators should appear in the schema
        for pred_type in &[
            "glob_match", "equals", "value_in_set", "value_not_in_set",
            "numeric_range", "time_window", "label_set_match",
            "file_glob_match", "and", "or", "not",
        ] {
            assert!(
                schema.contains(pred_type),
                "schema should include predicate type '{}'",
                pred_type
            );
        }
    }

    #[test]
    fn test_spec_validates_against_schema() {
        // Generate a spec and verify it matches the schema structure
        let checks = vec![FilterCheck {
            name: "title",
            predicate: Predicate::GlobMatch {
                fact: Fact::PrTitle,
                pattern: "test".into(),
            },
            build_tag_suffix: "title-mismatch",
        }];
        let spec = build_gate_spec(GateContext::PullRequest, &checks);
        let spec_json = serde_json::to_value(&spec).unwrap();

        // Verify structural expectations from schema
        assert!(spec_json["context"]["build_reason"].is_string());
        assert!(spec_json["facts"].is_array());
        assert!(spec_json["checks"].is_array());
        assert!(spec_json["checks"][0]["predicate"]["type"].as_str() == Some("glob_match"));
    }

    #[test]
    #[ignore] // Writes to source tree — run manually with `cargo test test_write_schema -- --ignored`
    fn test_write_schema_to_scripts() {
        // Generate schema and write to scripts/ for distribution
        let schema = generate_gate_spec_schema();
        let schema_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("scripts")
            .join("gate-spec.schema.json");
        std::fs::write(&schema_path, &schema)
            .expect("should write schema file");

        // Verify it's readable and valid
        let read_back = std::fs::read_to_string(&schema_path).unwrap();
        let _: serde_json::Value = serde_json::from_str(&read_back).unwrap();
    }
}

