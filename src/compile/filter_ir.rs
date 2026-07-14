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
///
/// SYNC: the deterministic trigger-condition E2E harness hand-maintains a
/// mirror of `kind()` / `failure_policy()` / `dependencies()` in
/// `scripts/ado-script/src/trigger-e2e/gate-spec.ts` (the `FACT_META` table) so
/// it can craft gate specs without invoking the compiler. This is machine-
/// guarded: `generate_fact_catalog` emits `fact-catalog.gen.json` (regenerated
/// by `npm run codegen`, CI drift-checked like `types.gen.ts`), and a
/// `gate-spec.test.ts` test deep-compares `FACT_META` against it — so a changed
/// `failure_policy`/`dependencies` or a new/removed `Fact` fails a unit test.
/// When adding a `Fact` variant, also append it to `Fact::ALL` (the exhaustive
/// match in `_fact_all_exhaustiveness_reminder` will not compile until you do).
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
    /// Only used for test assertions; the runtime evaluator has its own
    /// mirror in `scripts/ado-script/src/shared/env-facts.ts::isPipelineVarFact`.
    #[cfg(test)]
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

    /// Changed file glob matching via the external gate evaluator.
    FileGlobMatch {
        include: Vec<String>,
        exclude: Vec<String>,
    },

    /// Logical AND — all must pass.
    /// Not yet produced by lowering; reserved for future compound filters.
    #[allow(dead_code)]
    And(Vec<Predicate>),
    /// Logical OR — at least one must pass.
    /// Not yet produced by lowering; reserved for future compound filters.
    #[allow(dead_code)]
    Or(Vec<Predicate>),
    /// Logical NOT — inner must fail.
    /// Not yet produced by lowering; reserved for future compound filters.
    #[allow(dead_code)]
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
    /// Not yet produced by validation; reserved for future advisory
    /// diagnostics that should appear in the compile log without
    /// blocking or warning. Mirrors the And/Or/Not "reserved for future"
    /// pattern at the top of `Predicate`.
    #[allow(dead_code)]
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

/// Push a `GlobMatch` check for a `PatternFilter` field.
fn push_glob_match(
    checks: &mut Vec<FilterCheck>,
    name: &'static str,
    fact: Fact,
    filter: &super::types::PatternFilter,
    suffix: &'static str,
) {
    checks.push(FilterCheck {
        name,
        predicate: Predicate::GlobMatch { fact, pattern: filter.pattern.clone() },
        build_tag_suffix: suffix,
    });
}

/// Push include and/or exclude checks for an `IncludeExcludeFilter` against `fact`.
///
/// Only checks with non-empty value sets are emitted.
fn push_value_set_checks(
    checks: &mut Vec<FilterCheck>,
    fact: Fact,
    filter: &super::types::IncludeExcludeFilter,
    include_name: &'static str,
    include_suffix: &'static str,
    exclude_name: &'static str,
    exclude_suffix: &'static str,
) {
    if !filter.include.is_empty() {
        checks.push(FilterCheck {
            name: include_name,
            predicate: Predicate::ValueInSet {
                fact,
                values: filter.include.clone(),
                case_insensitive: true,
            },
            build_tag_suffix: include_suffix,
        });
    }
    if !filter.exclude.is_empty() {
        checks.push(FilterCheck {
            name: exclude_name,
            predicate: Predicate::ValueNotInSet {
                fact,
                values: filter.exclude.clone(),
                case_insensitive: true,
            },
            build_tag_suffix: exclude_suffix,
        });
    }
}

/// Lower `PrFilters` into a list of `FilterCheck` IR nodes.
pub fn lower_pr_filters(filters: &super::types::PrFilters) -> Vec<FilterCheck> {
    let mut checks = Vec::new();

    // Tier 1: Pipeline variables
    if let Some(title) = &filters.title {
        push_glob_match(&mut checks, "title", Fact::PrTitle, title, "title-mismatch");
    }

    if let Some(author) = &filters.author {
        push_value_set_checks(
            &mut checks,
            Fact::AuthorEmail,
            author,
            "author include",
            "author-mismatch",
            "author exclude",
            "author-excluded",
        );
    }

    if let Some(source) = &filters.source_branch {
        push_glob_match(&mut checks, "source-branch", Fact::SourceBranch, source, "source-branch-mismatch");
    }

    if let Some(target) = &filters.target_branch {
        push_glob_match(&mut checks, "target-branch", Fact::TargetBranch, target, "target-branch-mismatch");
    }

    if let Some(cm) = &filters.commit_message {
        push_glob_match(&mut checks, "commit-message", Fact::CommitMessage, cm, "commit-message-mismatch");
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
                value: if draft_expected { "true".into() } else { "false".into() },
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
            predicate: Predicate::TimeWindow { start: tw.start.clone(), end: tw.end.clone() },
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
        push_value_set_checks(
            &mut checks,
            Fact::BuildReason,
            br,
            "build-reason include",
            "build-reason-mismatch",
            "build-reason exclude",
            "build-reason-excluded",
        );
    }

    checks
}

/// Lower `PipelineFilters` into a list of `FilterCheck` IR nodes.
pub fn lower_pipeline_filters(filters: &super::types::PipelineFilters) -> Vec<FilterCheck> {
    let mut checks = Vec::new();

    if let Some(sp) = &filters.source_pipeline {
        push_glob_match(&mut checks, "source-pipeline", Fact::TriggeredByPipeline, sp, "source-pipeline-mismatch");
    }

    if let Some(branch) = &filters.branch {
        push_glob_match(&mut checks, "branch", Fact::TriggeringBranch, branch, "branch-mismatch");
    }

    if let Some(tw) = &filters.time_window {
        checks.push(FilterCheck {
            name: "time-window",
            predicate: Predicate::TimeWindow { start: tw.start.clone(), end: tw.end.clone() },
            build_tag_suffix: "time-window-mismatch",
        });
    }

    if let Some(br) = &filters.build_reason {
        push_value_set_checks(
            &mut checks,
            Fact::BuildReason,
            br,
            "build-reason include",
            "build-reason-mismatch",
            "build-reason exclude",
            "build-reason-excluded",
        );
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
    if let (Some(min), Some(max)) = (filters.min_changes, filters.max_changes)
        && min > max
    {
        diags.push(Diagnostic {
            severity: Severity::Error,
            filter: "min-changes / max-changes".into(),
            message: format!(
                "min-changes ({}) is greater than max-changes ({}) — no PR can satisfy both",
                min, max
            ),
        });
    }

    // Time window validation
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
pub fn validate_pipeline_filters(filters: &super::types::PipelineFilters) -> Vec<Diagnostic> {
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

use schemars::JsonSchema;
use serde::Serialize;

/// Serializable gate specification — the JSON document consumed by the
/// Node gate evaluator (`scripts/ado-script/gate.js`) at pipeline runtime.
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
    /// Kinds of other facts that must be acquired before this one.
    /// Mirrors `Fact::dependencies()`. Carried in the spec so the gate
    /// evaluator does not duplicate the dependency graph.
    pub dependencies: Vec<String>,
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
/// TypeScript gate evaluator. It is used to generate `types.gen.ts` in
/// the `scripts/ado-script` workspace.
pub fn generate_gate_spec_schema() -> String {
    let schema = schemars::schema_for!(GateSpec);
    serde_json::to_string_pretty(&schema).expect("schema serialization")
}

/// One entry in the machine-generated fact catalog.
#[derive(serde::Serialize)]
struct FactCatalogEntry {
    kind: &'static str,
    failure_policy: &'static str,
    dependencies: Vec<&'static str>,
}

/// Generate the fact catalog JSON: every [`Fact`]'s `kind`, `failure_policy`,
/// and (kind-named) `dependencies`, in declaration order.
///
/// This is the machine-verifiable source of truth for the hand-maintained
/// `FACT_META` mirror in
/// `scripts/ado-script/src/trigger-e2e/gate-spec.ts`. It is emitted to a
/// committed `fact-catalog.gen.json` by `npm run codegen` and drift-checked in
/// CI exactly like `types.gen.ts`, so a Rust-side change to a fact's policy or
/// dependencies (or a new/removed `Fact`) forces a catalog regen — which the
/// trigger-e2e unit test then flags against `FACT_META`.
pub fn generate_fact_catalog() -> String {
    let entries: Vec<FactCatalogEntry> = Fact::ALL
        .iter()
        .map(|f| FactCatalogEntry {
            kind: f.kind(),
            failure_policy: f.failure_policy().as_str(),
            dependencies: f.dependencies().iter().map(|d| d.kind()).collect(),
        })
        .collect();
    serde_json::to_string_pretty(&entries).expect("fact catalog serialization")
}

// ─── Codegen ────────────────────────────────────────────────────────────────

// The inline heredoc evaluator has been removed in favor of external script delivery.
// See AdoScriptExtension for the external path (bundled TypeScript gate.js).

impl Fact {
    /// ADO macro exports required by this fact.
    ///
    /// Returns `(env_var_name, ado_macro)` pairs that must be set in the
    /// step's `env:` block for the gate evaluator to read.
    ///
    /// **Drift note:** the TypeScript gate evaluator carries its own copy of
    /// this mapping in `scripts/ado-script/src/shared/env-facts.ts` as the
    /// `ENV_BY_FACT` table plus the `FactKind` type union. Those are *not*
    /// covered by the `types.gen.ts` codegen drift check (which only mirrors
    /// `GateSpec` shape), so when adding a new pipeline-variable `Fact`
    /// variant here you **must also** add an entry to `ENV_BY_FACT` and
    /// extend `FactKind`. Failing to do so produces a silent wrong-answer
    /// bug at runtime: `readEnvFact` returns `undefined`, the fact's failure
    /// policy decides the verdict, and a `fail_open` fact would let the gate
    /// pass without ever checking the predicate.
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
            // Always provided by infra vars in collect_ado_exports — no need to duplicate
            Fact::BuildReason => vec![],
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

    /// Every `Fact` variant, in declaration order.
    ///
    /// Kept complete by `_fact_all_exhaustiveness_reminder` below: adding a
    /// `Fact` variant is a compile error there until the variant is appended
    /// here, so `ALL` (and therefore the generated `fact-catalog.gen.json`)
    /// can never silently miss a fact.
    ///
    /// Maintenance: the `14` length literal is itself a second compile-time
    /// guard — appending a variant without bumping it is a mismatched-array-
    /// length error. Keep it equal to the number of `Fact` variants.
    pub const ALL: [Fact; 14] = [
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
}

/// Compile-time completeness guard for [`Fact::ALL`].
///
/// This wildcard-free match makes adding a `Fact` variant a hard compile error
/// until the new variant is listed. When you add the arm here, ALSO append the
/// variant to `Fact::ALL` above — the generated `fact-catalog.gen.json` (and the
/// TypeScript `FACT_META` mirror it guards) is derived from `Fact::ALL`.
#[allow(dead_code)]
fn _fact_all_exhaustiveness_reminder(f: Fact) {
    match f {
        Fact::PrTitle
        | Fact::AuthorEmail
        | Fact::SourceBranch
        | Fact::TargetBranch
        | Fact::CommitMessage
        | Fact::BuildReason
        | Fact::TriggeredByPipeline
        | Fact::TriggeringBranch
        | Fact::PrMetadata
        | Fact::PrIsDraft
        | Fact::PrLabels
        | Fact::ChangedFiles
        | Fact::ChangedFileCount
        | Fact::CurrentUtcMinutes => {}
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
pub fn build_gate_spec(ctx: GateContext, checks: &[FilterCheck]) -> anyhow::Result<GateSpec> {
    let facts_set = collect_ordered_facts(checks)?;

    let facts: Vec<FactSpec> = facts_set
        .iter()
        .map(|f| FactSpec {
            kind: f.kind().into(),
            failure_policy: f.failure_policy().as_str().into(),
            dependencies: f.dependencies().iter().map(|d| d.kind().into()).collect(),
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

    Ok(GateSpec {
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
    })
}

/// Compile filter checks into a bash gate step using an external evaluator
/// script. ADO variables are passed via the step's `env:` block (idiomatic
/// ADO pattern), and the gate spec is base64-encoded in GATE_SPEC.
///
/// When `synthetic_pr_active` is true AND `ctx == GateContext::PullRequest`,
/// PR-identifier env vars (`ADO_PR_ID`, `ADO_SOURCE_BRANCH`,
/// `ADO_TARGET_BRANCH`) read the canonical `AW_PR_*` same-job variables
/// that the preceding `synthPr` step emits via `setVar` — whether the
/// build is a real PR (synthPr copied `SYSTEM_PULLREQUEST_*`) or a
/// synth-promoted CI build (synthPr discovered + emitted the values).
/// Also exports `AW_SYNTHETIC_PR` so `gate/bypass.ts` knows to skip the
/// "not a PR build" bypass on synth-promoted builds.
///
/// **Same-job synth references**: this gate step lives in the **Setup
/// job** (`AdoScriptExtension::declarations` returns it), the same job
/// as `synthPr`. Three ADO behaviours interact here:
///
/// 1. The cross-job form `dependencies.Setup.outputs['synthPr.X']` is
///    undefined inside the producing job (a job has no entry for itself
///    in `dependencies`).
/// 2. Step output variables marked `isOutput=true` are NOT added to the
///    producing job's regular variable namespace, so `$(X)` and
///    `$[ variables['X'] ]` resolve to empty unless the producer ALSO
///    emits the same name as a regular (non-output) variable. The
///    `setVar` call in `exec-context-pr-synth/index.ts` is what makes
///    `$(AW_PR_*)` work here.
/// 3. The `$[ ... ]` runtime-expression form is NOT evaluated inside
///    step `env:` values — only inside `variables:` mappings and
///    `condition:` fields. Putting `$[ coalesce(...) ]` in step env
///    passes the literal expression string verbatim to the step
///    (msazuresphere/4x4 build #612528).
///
/// The fix is to do the real-vs-synth merge in `exec-context-pr-synth.js`
/// (which always runs and always emits the canonical `AW_PR_*` names),
/// and have this step consume them via plain `$(AW_PR_*)` macros —
/// reading the same-job regular variable that `setVar` registered.
/// See <https://learn.microsoft.com/en-us/azure/devops/pipelines/process/variables#use-output-variables-from-tasks>.
#[cfg(test)]
pub fn compile_gate_step_external(
    ctx: GateContext,
    checks: &[FilterCheck],
    evaluator_path: &str,
    synthetic_pr_active: bool,
) -> anyhow::Result<String> {
    use base64::{Engine as _, engine::general_purpose::STANDARD};

    if checks.is_empty() {
        return Ok(String::new());
    }

    let spec = build_gate_spec(ctx, checks)?;
    let spec_json = serde_json::to_string(&spec)?;
    let spec_b64 = STANDARD.encode(spec_json.as_bytes());

    let exports = collect_ado_exports(checks)?;

    let mut step = String::new();
    step.push_str(&format!("- bash: node '{}'\n", evaluator_path));
    step.push_str(&format!("  name: {}\n", ctx.step_name()));
    step.push_str(&format!("  displayName: \"{}\"\n", ctx.display_name()));
    step.push_str("  condition: succeeded()\n");
    step.push_str("  env:\n");
    // SYSTEM_ACCESSTOKEN is always needed for self-cancel (PATCH to builds API).
    // This uses the pipeline's built-in token, not an ARM service connection.
    // The build must have "Allow scripts to access the OAuth token" enabled.
    step.push_str("    SYSTEM_ACCESSTOKEN: $(System.AccessToken)\n");
    step.push_str(&format!("    GATE_SPEC: \"{}\"\n", spec_b64));

    // Synthetic-from-ci flag: tells gate/bypass.ts that this CI build
    // has been promoted to PR semantics, so the "not a PullRequest
    // build" bypass must not auto-pass. Read via the same-job macro
    // form — `synthPr` always runs and emits AW_SYNTHETIC_PR (=`"true"`
    // on synth promotion, empty on real PR / non-promoted CI) via
    // `setVar`, so `$(AW_SYNTHETIC_PR)` resolves cleanly without any
    // runtime expression.
    let pr_synth_active = synthetic_pr_active && matches!(ctx, GateContext::PullRequest);
    if pr_synth_active {
        step.push_str("    AW_SYNTHETIC_PR: $(AW_SYNTHETIC_PR)\n");
    }

    for (env_var, ado_macro) in &exports {
        let macro_str = if pr_synth_active {
            // Read the canonical `AW_PR_*` variables that `synthPr`
            // always emits via `setVar` (same-job). On real PR builds
            // `synthPr` copies `SYSTEM_PULLREQUEST_*` into them; on
            // synth-promoted CI builds it discovers + emits them. The
            // merge happens inside the bundle, so this step is blind
            // to the source — single macro form, single code path.
            match *env_var {
                "ADO_PR_ID" => "$(AW_PR_ID)",
                "ADO_SOURCE_BRANCH" => "$(AW_PR_SOURCEBRANCH)",
                "ADO_TARGET_BRANCH" => "$(AW_PR_TARGETBRANCH)",
                _ => ado_macro,
            }
        } else {
            ado_macro
        };
        step.push_str(&format!("    {}: {}\n", env_var, macro_str));
    }

    Ok(step)
}

// ─── Typed-IR gate step (port-ado-script) ───────────────────────────────

/// Constructs a typed IR gate step as a
/// [`crate::compile::ir::step::BashStep`] with `id` set to the
/// canonical gate step name (`prGate` / `pipelineGate`), typed
/// [`crate::compile::ir::condition::Condition::Succeeded`], and a
/// typed env block that uses
/// [`crate::compile::ir::env::EnvValue::StepOutput`] for cross-step
/// references and
/// [`crate::compile::ir::env::EnvValue::Concat`] for the
/// `$(System.PullRequest.X)$(synthPr.X)` mutually-empty macro-concat
/// pattern.
///
/// Lowering picks the right reference syntax per consumer location:
/// when the consumer is in the same job as `synthPr` (today's
/// production layout — gate + synthPr both live in Setup), the
/// `StepOutput` lowers to the macro form `$(synthPr.X)`. If a future
/// caller moves the gate to a different job, lowering would
/// auto-switch to `dependencies.Setup.outputs['synthPr.X']` without
/// any change to this builder — that is the whole point of the IR.
pub fn build_gate_step_typed(
    ctx: GateContext,
    checks: &[FilterCheck],
    evaluator_path: &str,
    synthetic_pr_active: bool,
) -> anyhow::Result<crate::compile::ir::step::BashStep> {
    use crate::compile::ado_bundle::{Bundle, TokenSource, apply_bundle_auth};
    use crate::compile::ir::condition::Condition;
    use crate::compile::ir::env::EnvValue;
    use crate::compile::ir::ids::StepId;
    use crate::compile::ir::step::BashStep;
    use base64::{Engine as _, engine::general_purpose::STANDARD};

    if checks.is_empty() {
        anyhow::bail!(
            "build_gate_step_typed called with empty checks — caller must \
             guard with !checks.is_empty() (matches compile_gate_step_external)"
        );
    }

    let spec = build_gate_spec(ctx, checks)?;
    let spec_json = serde_json::to_string(&spec)?;
    let spec_b64 = STANDARD.encode(spec_json.as_bytes());

    let exports = collect_ado_exports(checks)?;
    let pr_synth_active = synthetic_pr_active && matches!(ctx, GateContext::PullRequest);

    let script = format!("node '{evaluator_path}'\n");
    let mut step = apply_bundle_auth(
        BashStep::new(ctx.display_name(), script)
            .with_id(StepId::new(ctx.step_name())?)
            .with_condition(Condition::Succeeded)
            // The gate evaluator JS bundle emits `##vso[task.setvariable
            // variable=SHOULD_RUN;isOutput=true]` at runtime — declare it
            // here so cross-job consumers (e.g. the Agent-job condition's
            // typed `Condition::Eq(Expr::StepOutput(..., "SHOULD_RUN"))`)
            // pass graph validation. See `src/compile/ir/output.rs` for
            // the `OutputDecl` contract.
            .with_output(crate::compile::ir::output::OutputDecl::new("SHOULD_RUN")),
        Bundle::Gate,
        TokenSource::SystemAccessToken,
    )
    .with_env("GATE_SPEC", EnvValue::literal(spec_b64));

    // AW_SYNTHETIC_PR (same-job consumer of the synthPr step) reads
    // the setVar-registered variable via plain `$(name)` macro. The
    // `synthPr` step emits both `setOutput` (cross-job) and `setVar`
    // (same-job) for every value, so this is functionally equivalent
    // to `$(synthPr.AW_SYNTHETIC_PR)` at runtime but matches the
    // legacy emitter's wire form (which the regression test in
    // `tests/compiler_tests.rs::test_pr_filter_synth_mode_gate_step_uses_same_job_synth_ref`
    // pins).
    if pr_synth_active {
        step = step.with_env("AW_SYNTHETIC_PR", EnvValue::pipeline_var("AW_SYNTHETIC_PR"));
    }

    for (env_var, ado_macro) in &exports {
        let value = if pr_synth_active {
            match *env_var {
                // The three identifiers that change between real-PR
                // and synth-PR builds: read the unified `AW_PR_*`
                // job variable that `synthPr` always emits via
                // `setVar` (real on PR builds, discovered on
                // synth-promoted CI builds). The merge happens
                // inside the bundle, so this step reads a single
                // name regardless of source.
                "ADO_PR_ID" => EnvValue::pipeline_var("AW_PR_ID"),
                "ADO_SOURCE_BRANCH" => EnvValue::pipeline_var("AW_PR_SOURCEBRANCH"),
                "ADO_TARGET_BRANCH" => EnvValue::pipeline_var("AW_PR_TARGETBRANCH"),
                _ => env_value_from_ado_macro(env_var, ado_macro)?,
            }
        } else {
            env_value_from_ado_macro(env_var, ado_macro)?
        };
        step = step.with_env(*env_var, value);
    }

    Ok(step)
}

/// Map a legacy `(env_var, "$(Some.Macro)")` exports entry to a typed
/// [`crate::compile::ir::env::EnvValue`]. Predefined-variable macros
/// route through [`crate::compile::ir::env::EnvValue::ado_macro`] (so
/// the allowlist enforces no typos).
///
/// Anything that does not match the allowlist falls through to
/// [`crate::compile::ir::env::EnvValue::Literal`] with the raw
/// `$(X.Y)` string preserved. ADO substitutes the macro at runtime
/// either way, so emitted YAML is byte-identical to the allowlisted
/// path, but the fallback emits a compile-time `log::warn!` so a
/// new predefined-variable use site doesn't quietly accrete here —
/// extend [`crate::compile::ir::env::ALLOWED_ADO_MACROS`] when you
/// see this warning.
fn env_value_from_ado_macro(
    name: &str,
    ado_macro: &'static str,
) -> anyhow::Result<crate::compile::ir::env::EnvValue> {
    use crate::compile::ir::env::{ALLOWED_ADO_MACROS, EnvValue};

    // Unwrap `$(X.Y)` → `X.Y` for the allowlist lookup.
    let stripped = ado_macro
        .strip_prefix("$(")
        .and_then(|rest| rest.strip_suffix(')'));
    if let Some(inner) = stripped
        && ALLOWED_ADO_MACROS.contains(&inner)
    {
        // Promote the inner string to `&'static str` via the
        // allowlist entry so EnvValue::AdoMacro's static-lifetime
        // requirement is satisfied with the canonical reference.
        for allowed in ALLOWED_ADO_MACROS {
            if *allowed == inner {
                return EnvValue::ado_macro(allowed);
            }
        }
    }
    // Fallback: keep the raw scalar verbatim and surface the bypass
    // so it doesn't silently accrete.
    log::warn!(
        "filter_ir: env var {name:?} maps to ADO macro {ado_macro:?} which is not in \
         ALLOWED_ADO_MACROS. Emitting as a literal; consider adding it to the allowlist \
         in src/compile/ir/env.rs so EnvValue::AdoMacro can carry it typed."
    );
    Ok(EnvValue::literal(ado_macro))
}

// ─── PR synthetic-from-ci spec (mode: synthetic) ────────────────────────────

/// Base64-encoded JSON spec consumed by the `exec-context-pr-synth.js`
/// bundle at runtime. Carries the PR branch/path filters the agent
/// declared in front-matter so the bundle can match an active PR by
/// `sourceRefName` and filter by `targetRefName` + changed-file paths.
///
/// Shape:
/// ```json
/// {
///   "branches": { "include": [...], "exclude": [...] },
///   "paths":    { "include": [...], "exclude": [...] }
/// }
/// ```
///
/// All four arrays are always present (possibly empty) for shape stability —
/// the bundle can rely on the fields existing.
#[derive(Debug, serde::Serialize)]
struct PrSynthSpec {
    branches: PrSynthGlobs,
    paths: PrSynthGlobs,
}

#[derive(Debug, serde::Serialize)]
struct PrSynthGlobs {
    include: Vec<String>,
    exclude: Vec<String>,
}

/// Maximum decoded size of `PR_SYNTH_SPEC`. Matches the spirit of the
/// `GATE_SPEC` 8 KiB ceiling — synth specs are smaller (no checks, no
/// facts), but the same defence-in-depth bound prevents pathological
/// front-matter from blowing up the bundle's parser.
const PR_SYNTH_SPEC_MAX_BYTES: usize = 8 * 1024;

/// Build the base64-encoded `PR_SYNTH_SPEC` value for the given PR
/// trigger configuration.
///
/// The returned string is safe to embed inside a YAML double-quoted
/// scalar (the base64 alphabet contains no characters that require
/// YAML escaping).
pub fn build_pr_synth_spec(pr: &crate::compile::types::PrTriggerConfig) -> anyhow::Result<String> {
    use base64::{Engine as _, engine::general_purpose::STANDARD};

    let spec = PrSynthSpec {
        branches: PrSynthGlobs {
            include: pr
                .branches
                .as_ref()
                .map(|b| b.include.clone())
                .unwrap_or_default(),
            exclude: pr
                .branches
                .as_ref()
                .map(|b| b.exclude.clone())
                .unwrap_or_default(),
        },
        paths: PrSynthGlobs {
            include: pr
                .paths
                .as_ref()
                .map(|p| p.include.clone())
                .unwrap_or_default(),
            exclude: pr
                .paths
                .as_ref()
                .map(|p| p.exclude.clone())
                .unwrap_or_default(),
        },
    };

    let json = serde_json::to_string(&spec)?;
    anyhow::ensure!(
        json.len() <= PR_SYNTH_SPEC_MAX_BYTES,
        "PR_SYNTH_SPEC serialised size {} exceeds {}-byte cap; reduce the number/length of on.pr branches/paths globs",
        json.len(),
        PR_SYNTH_SPEC_MAX_BYTES
    );
    Ok(STANDARD.encode(json.as_bytes()))
}

/// Collect ADO macro exports needed by the given checks.
fn collect_ado_exports(
    checks: &[FilterCheck],
) -> anyhow::Result<Vec<(&'static str, &'static str)>> {
    let facts_set = collect_ordered_facts(checks)?;
    let mut exports: Vec<(&str, &str)> = Vec::new();
    let mut seen = BTreeSet::new();

    // Always-needed infra vars.
    // Collection URI is intentionally NOT exported here: ado-script reads
    // ADO's auto-injected SYSTEM_COLLECTIONURI directly (see auth.ts).
    let infra: Vec<(&str, &str)> = vec![
        ("ADO_BUILD_REASON", "$(Build.Reason)"),
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
    Ok(exports)
}

/// Collect all facts required by checks, topologically sorted so every
/// fact appears after its dependencies.
///
/// Uses an explicit topo-sort rather than relying on enum `Ord` ordering,
/// so the correctness does not depend on variant declaration order.
fn collect_ordered_facts(checks: &[FilterCheck]) -> anyhow::Result<Vec<Fact>> {
    let mut all_facts = BTreeSet::new();
    for check in checks {
        for fact in check.all_required_facts() {
            all_facts.insert(fact);
        }
    }

    // Kahn's algorithm: emit facts whose dependencies are already emitted.
    let mut remaining: Vec<Fact> = all_facts.into_iter().collect();
    let mut emitted = BTreeSet::new();
    let mut ordered = Vec::with_capacity(remaining.len());

    while !remaining.is_empty() {
        let before = remaining.len();
        remaining.retain(|fact| {
            let deps_met = fact.dependencies().iter().all(|dep| emitted.contains(dep));
            if deps_met {
                emitted.insert(*fact);
                ordered.push(*fact);
                false // remove from remaining
            } else {
                true // keep for next pass
            }
        });
        anyhow::ensure!(
            remaining.len() < before,
            "circular dependency detected in Fact graph — check Fact::dependencies()"
        );
    }

    Ok(ordered)
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::types::*;

    // ─── PR_SYNTH_SPEC tests ────────────────────────────────────────────

    #[test]
    fn test_build_pr_synth_spec_roundtrip() {
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        use serde_json::Value;

        let pr = PrTriggerConfig {
            branches: Some(BranchFilter {
                include: vec!["main".into(), "release/*".into()],
                exclude: vec!["test/*".into()],
            }),
            paths: Some(PathFilter {
                include: vec!["src/*".into()],
                exclude: vec!["docs/*".into()],
            }),
            filters: None,
            ..Default::default()
        };
        let b64 = build_pr_synth_spec(&pr).expect("synth spec must build");
        let decoded = STANDARD.decode(b64.as_bytes()).expect("must decode base64");
        let parsed: Value = serde_json::from_slice(&decoded).expect("must be valid JSON");
        assert_eq!(
            parsed["branches"]["include"],
            serde_json::json!(["main", "release/*"])
        );
        assert_eq!(parsed["branches"]["exclude"], serde_json::json!(["test/*"]));
        assert_eq!(parsed["paths"]["include"], serde_json::json!(["src/*"]));
        assert_eq!(parsed["paths"]["exclude"], serde_json::json!(["docs/*"]));
    }

    #[test]
    fn test_build_pr_synth_spec_omitted_arrays_become_empty() {
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        use serde_json::Value;

        let pr = PrTriggerConfig::default();
        let b64 = build_pr_synth_spec(&pr).expect("synth spec must build");
        let decoded = STANDARD.decode(b64.as_bytes()).unwrap();
        let parsed: Value = serde_json::from_slice(&decoded).unwrap();
        assert_eq!(parsed["branches"]["include"], serde_json::json!([]));
        assert_eq!(parsed["branches"]["exclude"], serde_json::json!([]));
        assert_eq!(parsed["paths"]["include"], serde_json::json!([]));
        assert_eq!(parsed["paths"]["exclude"], serde_json::json!([]));
    }

    #[test]
    fn test_build_pr_synth_spec_rejects_oversize() {
        // Generate enough branch globs to blow past the 8 KiB cap.
        let pr = PrTriggerConfig {
            branches: Some(BranchFilter {
                include: (0..1000)
                    .map(|i| format!("very/long/branch/glob/pattern/{i}"))
                    .collect(),
                exclude: vec![],
            }),
            paths: None,
            filters: None,
            ..Default::default()
        };
        let err = build_pr_synth_spec(&pr).expect_err("oversize spec must fail");
        assert!(
            err.to_string().contains("PR_SYNTH_SPEC"),
            "error must mention spec: {err}"
        );
    }

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
            Fact::TriggeredByPipeline,
            Fact::TriggeringBranch,
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
        // Iteration API: ChangedFileCount depends on ChangedFiles
        assert_eq!(Fact::ChangedFileCount.dependencies(), &[Fact::ChangedFiles]);
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
        let kinds: BTreeSet<&str> = all_facts.iter().map(|f| f.kind()).collect();
        assert_eq!(
            kinds.len(),
            all_facts.len(),
            "fact kind strings must be unique"
        );
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
        assert!(
            matches!(
                &checks[0].predicate,
                Predicate::ValueInSet { fact: Fact::AuthorEmail, values, .. }
                    if values == &["alice@corp.com"]
            ),
            "include should lower to ValueInSet on AuthorEmail"
        );
        assert_eq!(checks[1].name, "author exclude");
        assert!(
            matches!(
                &checks[1].predicate,
                Predicate::ValueNotInSet { fact: Fact::AuthorEmail, values, .. }
                    if values == &["bot@noreply.com"]
            ),
            "exclude should lower to ValueNotInSet on AuthorEmail"
        );
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
        assert_eq!(checks[0].name, "labels");
        assert!(
            matches!(
                &checks[0].predicate,
                Predicate::LabelSetMatch { any_of, all_of, none_of }
                    if any_of == &["run-agent"] && all_of.is_empty() && none_of == &["do-not-run"]
            ),
            "labels should lower to LabelSetMatch preserving any_of and none_of"
        );
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
        assert_eq!(checks[0].name, "change-count");
        assert!(matches!(
            &checks[0].predicate,
            Predicate::NumericRange {
                fact: Fact::ChangedFileCount,
                min: Some(5),
                max: Some(100),
            }
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
        assert!(
            matches!(
                &checks[0].predicate,
                Predicate::GlobMatch { fact: Fact::TriggeredByPipeline, pattern }
                    if pattern == "Build.*"
            ),
            "source-pipeline should lower to GlobMatch on TriggeredByPipeline"
        );
        assert_eq!(checks[1].name, "branch");
        assert!(
            matches!(
                &checks[1].predicate,
                Predicate::GlobMatch { fact: Fact::TriggeringBranch, pattern }
                    if pattern == "^refs/heads/main$"
            ),
            "branch should lower to GlobMatch on TriggeringBranch"
        );
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
        assert!(
            diags
                .iter()
                .any(|d| d.severity == Severity::Error && d.filter == "min-changes / max-changes")
        );
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
        assert!(
            diags
                .iter()
                .any(|d| d.severity == Severity::Error && d.filter == "time-window")
        );
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
        assert!(
            diags
                .iter()
                .any(|d| d.severity == Severity::Error && d.filter == "author")
        );
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
        assert!(
            diags
                .iter()
                .any(|d| d.severity == Severity::Error && d.filter == "labels")
        );
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
        assert!(
            diags
                .iter()
                .any(|d| d.severity == Severity::Error && d.filter == "labels")
        );
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
        assert!(
            diags
                .iter()
                .any(|d| d.severity == Severity::Error && d.filter == "build-reason")
        );
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
        let result = compile_gate_step_external(
            GateContext::PullRequest,
            &[],
            "/tmp/ado-aw-scripts/ado-script/gate.js",
            false,
        )
        .unwrap();
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
        let result = compile_gate_step_external(
            GateContext::PullRequest,
            &checks,
            "/tmp/ado-aw-scripts/ado-script/gate.js",
            false,
        )
        .unwrap();
        assert!(result.contains("- bash:"), "should be a bash step");
        assert!(
            result.contains("GATE_SPEC"),
            "should include base64 spec in env"
        );
        assert!(
            result.contains("node '/tmp/ado-aw-scripts/ado-script/gate.js'"),
            "should reference external evaluator script"
        );
        assert!(result.contains("name: prGate"), "should set step name");
        assert!(
            result.contains("SYSTEM_ACCESSTOKEN"),
            "should pass access token via env block"
        );
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
        let result = compile_gate_step_external(
            GateContext::PullRequest,
            &checks,
            "/tmp/ado-aw-scripts/ado-script/gate.js",
            false,
        )
        .unwrap();
        assert!(
            result.contains("ADO_BUILD_REASON"),
            "should export build reason"
        );
        assert!(result.contains("ADO_PR_TITLE"), "should export PR title");
        assert!(
            result.contains("$(System.PullRequest.Title)"),
            "should reference ADO macro"
        );
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
        let result = compile_gate_step_external(
            GateContext::PipelineCompletion,
            &checks,
            "/tmp/ado-aw-scripts/ado-script/gate.js",
            false,
        )
        .unwrap();
        assert!(
            result.contains("name: pipelineGate"),
            "should set pipeline gate name"
        );
        assert!(
            result.contains("Evaluate pipeline filters"),
            "should set display name"
        );
        assert!(
            result.contains("ADO_TRIGGERED_BY_PIPELINE"),
            "should export pipeline macro"
        );
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
        let result = compile_gate_step_external(
            GateContext::PullRequest,
            &checks,
            "/tmp/ado-aw-scripts/ado-script/gate.js",
            false,
        )
        .unwrap();
        assert!(
            result.contains("ADO_REPO_ID"),
            "should export repo ID for API calls"
        );
        assert!(
            result.contains("ADO_PR_ID"),
            "should export PR ID for API calls"
        );
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
        let result = compile_gate_step_external(
            GateContext::PullRequest,
            &checks,
            "/tmp/ado-aw-scripts/ado-script/gate.js",
            false,
        )
        .unwrap();
        // Verify tier-1 (pipeline-var only) checks do not export API-related env vars
        // Look for the env: block exports (YAML format with leading spaces)
        let lines: Vec<&str> = result.lines().collect();
        let has_repo_id_export = lines
            .iter()
            .any(|line| line.trim_start().starts_with("ADO_REPO_ID:"));
        let has_pr_id_export = lines
            .iter()
            .any(|line| line.trim_start().starts_with("ADO_PR_ID:"));
        assert!(
            !has_repo_id_export,
            "should not export ADO_REPO_ID for title-only (tier-1) check"
        );
        assert!(
            !has_pr_id_export,
            "should not export ADO_PR_ID for title-only (tier-1) check"
        );
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
        let spec = build_gate_spec(GateContext::PullRequest, &checks).unwrap();
        assert_eq!(spec.context.build_reason, "PullRequest");
        assert_eq!(spec.context.tag_prefix, "pr-gate");
        assert_eq!(spec.context.step_name, "prGate");
        assert_eq!(spec.context.bypass_label, "PR");
        // Facts should include pr_title, pr_metadata (dep of pr_labels), pr_labels
        assert_eq!(spec.facts.len(), 3, "exactly 3 facts required for title + labels checks");
        assert!(spec.facts.iter().any(|f| f.kind == "pr_title"));
        assert!(spec.facts.iter().any(|f| f.kind == "pr_metadata"));
        assert!(spec.facts.iter().any(|f| f.kind == "pr_labels"));
        // Checks
        assert_eq!(spec.checks.len(), 2);
        assert_eq!(spec.checks[0].name, "title");
        assert_eq!(spec.checks[1].name, "labels");
    }

    #[test]
    fn test_tag_prefix_has_no_colon() {
        // Build tags are PUT into the ADO REST request path
        // (…/builds/<id>/tags/<tag>); a ':' trips ASP.NET's
        // dangerous-request-path validator and fails the whole build.
        // The gate composes tags as `${tag_prefix}.${suffix}`, so a colon
        // in tag_prefix itself would reintroduce the rejected character.
        for ctx in [GateContext::PullRequest, GateContext::PipelineCompletion] {
            assert!(
                !ctx.tag_prefix().contains(':'),
                "tag_prefix {:?} must not contain ':'",
                ctx.tag_prefix(),
            );
        }
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
        let spec = build_gate_spec(GateContext::PullRequest, &checks).unwrap();
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

        let step = compile_gate_step_external(
            GateContext::PullRequest,
            &checks,
            "/tmp/ado-aw-scripts/ado-script/gate.js",
            false,
        )
        .unwrap();

        // Tier-2 facts (PrIsDraft, PrLabels) require API calls — env vars must be present
        assert!(
            step.contains("ADO_PR_ID"),
            "draft/labels filters require ADO_PR_ID for API calls"
        );
        assert!(
            step.contains("ADO_REPO_ID"),
            "draft/labels filters require ADO_REPO_ID for API calls"
        );
        assert!(
            step.contains("GATE_SPEC"),
            "step should embed the serialised gate spec"
        );

        // Verify the spec captures all three filters with correct fact dependencies
        let spec = build_gate_spec(GateContext::PullRequest, &checks).unwrap();
        assert_eq!(
            spec.checks.len(),
            3,
            "should produce 3 checks from 3 filters"
        );
        assert!(
            spec.facts.iter().any(|f| f.kind == "pr_title"),
            "title filter requires pr_title fact"
        );
        assert!(
            spec.facts.iter().any(|f| f.kind == "pr_is_draft"),
            "draft filter requires pr_is_draft fact"
        );
        assert!(
            spec.facts.iter().any(|f| f.kind == "pr_labels"),
            "labels filter requires pr_labels fact"
        );
        assert!(
            spec.facts.iter().any(|f| f.kind == "pr_metadata"),
            "API-derived facts should pull in pr_metadata dependency"
        );
    }

    // ─── Schema tests ──────────────────────────────────────────────────

    #[test]
    fn test_generate_schema_is_valid_json() {
        let schema = generate_gate_spec_schema();
        let parsed: serde_json::Value =
            serde_json::from_str(&schema).expect("schema should be valid JSON");
        assert!(parsed.is_object());
        assert!(
            parsed.get("$schema").is_some() || parsed.get("type").is_some(),
            "should be a JSON Schema document"
        );
    }

    #[test]
    fn test_schema_includes_all_predicate_types() {
        let schema = generate_gate_spec_schema();
        // All predicate type discriminators should appear in the schema
        for pred_type in &[
            "glob_match",
            "equals",
            "value_in_set",
            "value_not_in_set",
            "numeric_range",
            "time_window",
            "label_set_match",
            "file_glob_match",
            "and",
            "or",
            "not",
        ] {
            assert!(
                schema.contains(pred_type),
                "schema should include predicate type '{}'",
                pred_type
            );
        }
    }

    #[test]
    #[ignore] // Writes to source tree — run manually with `cargo test test_write_schema -- --ignored`
    fn test_write_schema_to_scripts() {
        // Generate schema and write to the canonical location for codegen
        let schema = generate_gate_spec_schema();
        let schema_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("scripts")
            .join("ado-script")
            .join("schema")
            .join("gate-spec.schema.json");
        std::fs::create_dir_all(schema_path.parent().unwrap()).expect("should create schema dir");
        std::fs::write(&schema_path, &schema).expect("should write schema file");

        // Verify it's readable and valid
        let read_back = std::fs::read_to_string(&schema_path).unwrap();
        let _: serde_json::Value = serde_json::from_str(&read_back).unwrap();
    }
}
