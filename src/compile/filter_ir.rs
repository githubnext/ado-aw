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

use super::pr_filters::shell_escape;

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
            Fact::ChangedFileCount => &[], // may come from ChangedFiles or fresh fetch

            // Computed
            Fact::CurrentUtcMinutes => &[],
        }
    }

    /// Shell variable name this fact is stored in.
    pub fn shell_var(&self) -> &'static str {
        match self {
            Fact::PrTitle => "TITLE",
            Fact::AuthorEmail => "AUTHOR",
            Fact::SourceBranch => "SOURCE_BRANCH",
            Fact::TargetBranch => "TARGET_BRANCH",
            Fact::CommitMessage => "COMMIT_MSG",
            Fact::BuildReason => "REASON",
            Fact::TriggeredByPipeline => "SOURCE_PIPELINE",
            Fact::TriggeringBranch => "TRIGGER_BRANCH",
            Fact::PrMetadata => "PR_DATA",
            Fact::PrIsDraft => "IS_DRAFT",
            Fact::PrLabels => "PR_LABELS",
            Fact::ChangedFiles => "CHANGED_FILES",
            Fact::ChangedFileCount => "FILE_COUNT",
            Fact::CurrentUtcMinutes => "CURRENT_MINUTES",
        }
    }

    /// Bash snippet to acquire this fact. Indented with 4 spaces for
    /// embedding inside the gate step.
    pub fn acquisition_bash(&self) -> String {
        match self {
            // Pipeline variables — simple assignment from ADO macro
            Fact::PrTitle => "    TITLE=\"$(System.PullRequest.Title)\"".into(),
            Fact::AuthorEmail => "    AUTHOR=\"$(Build.RequestedForEmail)\"".into(),
            Fact::SourceBranch => {
                "    SOURCE_BRANCH=\"$(System.PullRequest.SourceBranch)\"".into()
            }
            Fact::TargetBranch => {
                "    TARGET_BRANCH=\"$(System.PullRequest.TargetBranch)\"".into()
            }
            Fact::CommitMessage => "    COMMIT_MSG=\"$(Build.SourceVersionMessage)\"".into(),
            Fact::BuildReason => "    REASON=\"$(Build.Reason)\"".into(),
            Fact::TriggeredByPipeline => {
                "    SOURCE_PIPELINE=\"$(Build.TriggeredBy.DefinitionName)\"".into()
            }
            Fact::TriggeringBranch => "    TRIGGER_BRANCH=\"$(Build.SourceBranch)\"".into(),

            // REST API — fetch full PR metadata
            Fact::PrMetadata => concat!(
                "    # Fetch PR metadata via REST API\n",
                "    PR_ID=\"$(System.PullRequest.PullRequestId)\"\n",
                "    ORG_URL=\"$(System.CollectionUri)\"\n",
                "    PROJECT=\"$(System.TeamProject)\"\n",
                "    REPO_ID=\"$(Build.Repository.ID)\"\n",
                "    PR_DATA=$(curl -s \\\n",
                "      -H \"Authorization: Bearer $SYSTEM_ACCESSTOKEN\" \\\n",
                "      \"${ORG_URL}${PROJECT}/_apis/git/repositories/${REPO_ID}/pullRequests/${PR_ID}?api-version=7.1\")\n",
                "    if [ -z \"$PR_DATA\" ] || echo \"$PR_DATA\" | python3 -c \"import sys,json; json.load(sys.stdin)\" 2>/dev/null; [ $? -ne 0 ] 2>/dev/null; then\n",
                "      echo \"##[warning]Failed to fetch PR data from API — skipping API-based filters\"\n",
                "    fi",
            )
            .into(),

            // Extract isDraft from PR metadata
            Fact::PrIsDraft => concat!(
                "    IS_DRAFT=$(echo \"$PR_DATA\" | python3 -c ",
                "\"import sys,json; print(str(json.load(sys.stdin).get('isDraft',False)).lower())\" ",
                "2>/dev/null || echo 'unknown')",
            )
            .into(),

            // Extract labels from PR metadata
            Fact::PrLabels => concat!(
                "    # Extract PR labels\n",
                "    PR_LABELS=$(echo \"$PR_DATA\" | python3 -c ",
                "\"import sys,json; data=json.load(sys.stdin); print('\\n'.join(l.get('name','') for l in data.get('labels',[])))\" ",
                "2>/dev/null || echo '')\n",
                "    echo \"PR labels: $PR_LABELS\"",
            )
            .into(),

            // Changed files via iterations API
            Fact::ChangedFiles => concat!(
                "    # Fetch changed files via PR iterations API\n",
                "    if [ -z \"${PR_ID:-}\" ]; then\n",
                "      PR_ID=\"$(System.PullRequest.PullRequestId)\"\n",
                "      ORG_URL=\"$(System.CollectionUri)\"\n",
                "      PROJECT=\"$(System.TeamProject)\"\n",
                "      REPO_ID=\"$(Build.Repository.ID)\"\n",
                "    fi\n",
                "    ITERATIONS=$(curl -s \\\n",
                "      -H \"Authorization: Bearer $SYSTEM_ACCESSTOKEN\" \\\n",
                "      \"${ORG_URL}${PROJECT}/_apis/git/repositories/${REPO_ID}/pullRequests/${PR_ID}/iterations?api-version=7.1\")\n",
                "    LAST_ITER=$(echo \"$ITERATIONS\" | python3 -c \"import sys,json; iters=json.load(sys.stdin).get('value',[]); print(iters[-1]['id'] if iters else '')\" 2>/dev/null || echo '')\n",
                "    if [ -n \"$LAST_ITER\" ]; then\n",
                "      CHANGES=$(curl -s \\\n",
                "        -H \"Authorization: Bearer $SYSTEM_ACCESSTOKEN\" \\\n",
                "        \"${ORG_URL}${PROJECT}/_apis/git/repositories/${REPO_ID}/pullRequests/${PR_ID}/iterations/${LAST_ITER}/changes?api-version=7.1\")\n",
                "      CHANGED_FILES=$(echo \"$CHANGES\" | python3 -c \"\n",
                "import sys, json\n",
                "data = json.load(sys.stdin)\n",
                "for entry in data.get('changeEntries', []):\n",
                "    item = entry.get('item', {})\n",
                "    path = item.get('path', '')\n",
                "    if path:\n",
                "        print(path.lstrip('/'))\n",
                "\" 2>/dev/null || echo '')\n",
                "    else\n",
                "      CHANGED_FILES=''\n",
                "      echo \"##[warning]Could not determine PR iterations for changed-files filter\"\n",
                "    fi\n",
                "    echo \"Changed files: $(echo \"$CHANGED_FILES\" | head -20)\"",
            )
            .into(),

            // Count from changed files data
            Fact::ChangedFileCount => {
                "    FILE_COUNT=$(echo \"$CHANGED_FILES\" | grep -c . || echo '0')\n    echo \"Changed file count: $FILE_COUNT\"".into()
            }

            // Current UTC time in minutes
            Fact::CurrentUtcMinutes => concat!(
                "    CURRENT_HOUR=$(date -u +%H)\n",
                "    CURRENT_MIN=$(date -u +%M)\n",
                "    CURRENT_MINUTES=$((CURRENT_HOUR * 60 + CURRENT_MIN))",
            )
            .into(),
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
    /// Regex match: `echo "$var" | grep -qE 'pattern'`
    RegexMatch { fact: Fact, pattern: String },

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
            Predicate::RegexMatch { fact, .. }
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
            predicate: Predicate::RegexMatch {
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
            predicate: Predicate::RegexMatch {
                fact: Fact::SourceBranch,
                pattern: source.pattern.clone(),
            },
            build_tag_suffix: "source-branch-mismatch",
        });
    }

    if let Some(target) = &filters.target_branch {
        checks.push(FilterCheck {
            name: "target-branch",
            predicate: Predicate::RegexMatch {
                fact: Fact::TargetBranch,
                pattern: target.pattern.clone(),
            },
            build_tag_suffix: "target-branch-mismatch",
        });
    }

    if let Some(cm) = &filters.commit_message {
        checks.push(FilterCheck {
            name: "commit-message",
            predicate: Predicate::RegexMatch {
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
            predicate: Predicate::RegexMatch {
                fact: Fact::TriggeredByPipeline,
                pattern: sp.pattern.clone(),
            },
            build_tag_suffix: "source-pipeline-mismatch",
        });
    }

    if let Some(branch) = &filters.branch {
        checks.push(FilterCheck {
            name: "branch",
            predicate: Predicate::RegexMatch {
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

    // Time window start == end
    if let Some(tw) = &filters.time_window {
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

// ─── Codegen ────────────────────────────────────────────────────────────────

/// Compile filter checks into a bash gate step.
///
/// The generated step:
/// 1. Bypasses non-matching trigger types automatically
/// 2. Acquires all required facts (dependency-ordered)
/// 3. Evaluates each predicate, setting SHOULD_RUN=false on failure
/// 4. Self-cancels the build via ADO REST API if any filter fails
pub fn compile_gate_step(ctx: GateContext, checks: &[FilterCheck]) -> String {
    if checks.is_empty() {
        return String::new();
    }

    // Collect and topo-sort required facts
    let facts = collect_ordered_facts(checks);

    let mut step = String::new();
    step.push_str("- bash: |\n");

    // Bypass for non-matching trigger types
    step.push_str(&format!(
        "    if [ \"$(Build.Reason)\" != \"{}\" ]; then\n",
        ctx.build_reason()
    ));
    step.push_str(&format!(
        "      echo \"Not a {} build -- gate passes automatically\"\n",
        match ctx {
            GateContext::PullRequest => "PR",
            GateContext::PipelineCompletion => "pipeline",
        }
    ));
    step.push_str(
        "      echo \"##vso[task.setvariable variable=SHOULD_RUN;isOutput=true]true\"\n",
    );
    step.push_str(&format!(
        "      echo \"##vso[build.addbuildtag]{}:passed\"\n",
        ctx.tag_prefix()
    ));
    step.push_str("      exit 0\n");
    step.push_str("    fi\n");
    step.push('\n');
    step.push_str("    SHOULD_RUN=true\n");

    // Acquire all facts
    for fact in &facts {
        step.push('\n');
        step.push_str(&fact.acquisition_bash());
        step.push('\n');
    }

    // Evaluate each predicate
    for check in checks {
        step.push('\n');
        emit_predicate_check(&mut step, check, ctx.tag_prefix());
    }

    step.push('\n');

    // Result handling
    step.push_str(
        "    echo \"##vso[task.setvariable variable=SHOULD_RUN;isOutput=true]$SHOULD_RUN\"\n",
    );
    step.push_str("    if [ \"$SHOULD_RUN\" = \"true\" ]; then\n");
    step.push_str("      echo \"All filters passed -- agent will run\"\n");
    step.push_str(&format!(
        "      echo \"##vso[build.addbuildtag]{}:passed\"\n",
        ctx.tag_prefix()
    ));
    step.push_str("    else\n");
    step.push_str("      echo \"Filters not matched -- cancelling build\"\n");
    step.push_str(&format!(
        "      echo \"##vso[build.addbuildtag]{}:skipped\"\n",
        ctx.tag_prefix()
    ));
    step.push_str("      curl -s -X PATCH \\\n");
    step.push_str(
        "        -H \"Authorization: Bearer $SYSTEM_ACCESSTOKEN\" \\\n",
    );
    step.push_str("        -H \"Content-Type: application/json\" \\\n");
    step.push_str("        -d '{\"status\": \"cancelling\"}' \\\n");
    step.push_str("        \"$(System.CollectionUri)$(System.TeamProject)/_apis/build/builds/$(Build.BuildId)?api-version=7.1\"\n");
    step.push_str("    fi\n");
    step.push_str(&format!("  name: {}\n", ctx.step_name()));
    step.push_str(&format!(
        "  displayName: \"{}\"\n",
        ctx.display_name()
    ));
    step.push_str("  env:\n");
    step.push_str("    SYSTEM_ACCESSTOKEN: $(System.AccessToken)");

    step
}

/// Collect all facts required by checks, topo-sorted by dependencies.
fn collect_ordered_facts(checks: &[FilterCheck]) -> Vec<Fact> {
    let mut all_facts = BTreeSet::new();
    for check in checks {
        for fact in check.all_required_facts() {
            all_facts.insert(fact);
        }
    }

    // Topo-sort: pipeline vars first, then API, then computed.
    // BTreeSet gives us Ord-based ordering which matches our enum variant order
    // (pipeline vars < API-derived < computed), so just collect.
    all_facts.into_iter().collect()
}

/// Emit bash for a single predicate check.
fn emit_predicate_check(out: &mut String, check: &FilterCheck, tag_prefix: &str) {
    let tag = format!("{}:{}", tag_prefix, check.build_tag_suffix);
    match &check.predicate {
        Predicate::RegexMatch { fact, pattern } => {
            let escaped = shell_escape(pattern);
            let var = fact.shell_var();
            out.push_str(&format!("    # {} filter\n", capitalize(check.name)));
            out.push_str(&format!(
                "    if echo \"${}\" | grep -qE '{}'; then\n",
                var, escaped
            ));
            out.push_str(&format!(
                "      echo \"Filter: {} | Pattern: {} | Result: PASS\"\n",
                check.name, escaped
            ));
            out.push_str("    else\n");
            out.push_str(&format!(
                "      echo \"##[warning]Filter {} did not match (pattern: {})\"\n",
                check.name, escaped
            ));
            out.push_str(&format!(
                "      echo \"##vso[build.addbuildtag]{}\"\n",
                tag
            ));
            out.push_str("      SHOULD_RUN=false\n");
            out.push_str("    fi\n");
        }

        Predicate::Equality { fact, value } => {
            let var = fact.shell_var();
            out.push_str(&format!("    # {} filter\n", capitalize(check.name)));
            out.push_str(&format!(
                "    if [ \"${}\" = \"{}\" ]; then\n",
                var, value
            ));
            out.push_str(&format!(
                "      echo \"Filter: {} | Expected: {} | Actual: ${}  | Result: PASS\"\n",
                check.name, value, var
            ));
            out.push_str("    else\n");
            out.push_str(&format!(
                "      echo \"##[warning]Filter {} did not match (expected: {}, actual: ${})\"\n",
                check.name, value, var
            ));
            out.push_str(&format!(
                "      echo \"##vso[build.addbuildtag]{}\"\n",
                tag
            ));
            out.push_str("      SHOULD_RUN=false\n");
            out.push_str("    fi\n");
        }

        Predicate::ValueInSet {
            fact,
            values,
            case_insensitive,
        } => {
            let var = fact.shell_var();
            let escaped: Vec<String> = values.iter().map(|v| shell_escape(v)).collect();
            let pattern = escaped.join("|");
            let flag = if *case_insensitive { "i" } else { "" };
            out.push_str(&format!("    # {} filter\n", capitalize(check.name)));
            out.push_str(&format!(
                "    if echo \"${}\" | grep -q{}E '^({})$'; then\n",
                var, flag, pattern
            ));
            out.push_str(&format!(
                "      echo \"Filter: {} | Result: PASS\"\n",
                check.name
            ));
            out.push_str("    else\n");
            out.push_str(&format!(
                "      echo \"##[warning]Filter {} did not match include list\"\n",
                check.name
            ));
            out.push_str(&format!(
                "      echo \"##vso[build.addbuildtag]{}\"\n",
                tag
            ));
            out.push_str("      SHOULD_RUN=false\n");
            out.push_str("    fi\n");
        }

        Predicate::ValueNotInSet {
            fact,
            values,
            case_insensitive,
        } => {
            let var = fact.shell_var();
            let escaped: Vec<String> = values.iter().map(|v| shell_escape(v)).collect();
            let pattern = escaped.join("|");
            let flag = if *case_insensitive { "i" } else { "" };
            out.push_str(&format!("    # {} filter\n", capitalize(check.name)));
            out.push_str(&format!(
                "    if echo \"${}\" | grep -q{}E '^({})$'; then\n",
                var, flag, pattern
            ));
            out.push_str(&format!(
                "      echo \"##[warning]Filter {} matched exclude list\"\n",
                check.name
            ));
            out.push_str(&format!(
                "      echo \"##vso[build.addbuildtag]{}\"\n",
                tag
            ));
            out.push_str("      SHOULD_RUN=false\n");
            out.push_str("    else\n");
            out.push_str(&format!(
                "      echo \"Filter: {} | Result: PASS (not in exclude list)\"\n",
                check.name
            ));
            out.push_str("    fi\n");
        }

        Predicate::NumericRange { fact: _, min, max } => {
            out.push_str(&format!("    # {} filter\n", capitalize(check.name)));
            if let Some(min_val) = min {
                out.push_str(&format!(
                    "    if [ \"$FILE_COUNT\" -ge {} ]; then\n",
                    min_val
                ));
                out.push_str(&format!(
                    "      echo \"Filter: min-changes | Min: {} | Actual: $FILE_COUNT | Result: PASS\"\n",
                    min_val
                ));
                out.push_str("    else\n");
                out.push_str(&format!(
                    "      echo \"##[warning]Filter min-changes: $FILE_COUNT files changed, minimum {} required\"\n",
                    min_val
                ));
                out.push_str(&format!(
                    "      echo \"##vso[build.addbuildtag]{}:min-{}\"\n",
                    tag_prefix, check.build_tag_suffix
                ));
                out.push_str("      SHOULD_RUN=false\n");
                out.push_str("    fi\n");
            }
            if let Some(max_val) = max {
                out.push_str(&format!(
                    "    if [ \"$FILE_COUNT\" -le {} ]; then\n",
                    max_val
                ));
                out.push_str(&format!(
                    "      echo \"Filter: max-changes | Max: {} | Actual: $FILE_COUNT | Result: PASS\"\n",
                    max_val
                ));
                out.push_str("    else\n");
                out.push_str(&format!(
                    "      echo \"##[warning]Filter max-changes: $FILE_COUNT files changed, maximum {} allowed\"\n",
                    max_val
                ));
                out.push_str(&format!(
                    "      echo \"##vso[build.addbuildtag]{}:max-{}\"\n",
                    tag_prefix, check.build_tag_suffix
                ));
                out.push_str("      SHOULD_RUN=false\n");
                out.push_str("    fi\n");
            }
        }

        Predicate::TimeWindow { start, end } => {
            let s = shell_escape(start);
            let e = shell_escape(end);
            out.push_str(&format!("    # {} filter\n", capitalize(check.name)));
            out.push_str(&format!("    START_H=${{{}%%:*}}\n", s));
            out.push_str(&format!("    START_M=${{{}##*:}}\n", s));
            out.push_str(
                "    START_MINUTES=$((10#$START_H * 60 + 10#$START_M))\n",
            );
            out.push_str(&format!("    END_H=${{{}%%:*}}\n", e));
            out.push_str(&format!("    END_M=${{{}##*:}}\n", e));
            out.push_str("    END_MINUTES=$((10#$END_H * 60 + 10#$END_M))\n");
            out.push_str("    if [ $START_MINUTES -le $END_MINUTES ]; then\n");
            out.push_str("      # Same-day window\n");
            out.push_str("      if [ $CURRENT_MINUTES -ge $START_MINUTES ] && [ $CURRENT_MINUTES -lt $END_MINUTES ]; then\n");
            out.push_str("        IN_WINDOW=true\n");
            out.push_str("      else\n");
            out.push_str("        IN_WINDOW=false\n");
            out.push_str("      fi\n");
            out.push_str("    else\n");
            out.push_str("      # Overnight window (e.g., 22:00-06:00)\n");
            out.push_str("      if [ $CURRENT_MINUTES -ge $START_MINUTES ] || [ $CURRENT_MINUTES -lt $END_MINUTES ]; then\n");
            out.push_str("        IN_WINDOW=true\n");
            out.push_str("      else\n");
            out.push_str("        IN_WINDOW=false\n");
            out.push_str("      fi\n");
            out.push_str("    fi\n");
            out.push_str("    if [ \"$IN_WINDOW\" = \"true\" ]; then\n");
            out.push_str(&format!(
                "      echo \"Filter: time-window | Window: {}-{} UTC | Result: PASS\"\n",
                s, e
            ));
            out.push_str("    else\n");
            out.push_str(&format!(
                "      echo \"##[warning]Filter time-window: current time is outside {}-{} UTC\"\n",
                s, e
            ));
            out.push_str(&format!(
                "      echo \"##vso[build.addbuildtag]{}\"\n",
                tag
            ));
            out.push_str("      SHOULD_RUN=false\n");
            out.push_str("    fi\n");
        }

        Predicate::LabelSetMatch {
            any_of,
            all_of,
            none_of,
        } => {
            out.push_str("    # Labels filter\n");

            if !any_of.is_empty() {
                let escaped: Vec<String> =
                    any_of.iter().map(|l| shell_escape(l)).collect();
                out.push_str("    LABEL_MATCH=false\n");
                for label in &escaped {
                    out.push_str(&format!(
                        "    if echo \"$PR_LABELS\" | grep -qiF '{}'; then\n",
                        label
                    ));
                    out.push_str("      LABEL_MATCH=true\n");
                    out.push_str("    fi\n");
                }
                out.push_str("    if [ \"$LABEL_MATCH\" = \"true\" ]; then\n");
                out.push_str(
                    "      echo \"Filter: labels any-of | Result: PASS\"\n"
                );
                out.push_str("    else\n");
                out.push_str(&format!(
                    "      echo \"##[warning]Filter labels any-of did not match (required one of: {})\"\n",
                    escaped.join(", ")
                ));
                out.push_str(&format!(
                    "      echo \"##vso[build.addbuildtag]{}\"\n",
                    tag
                ));
                out.push_str("      SHOULD_RUN=false\n");
                out.push_str("    fi\n");
            }

            if !all_of.is_empty() {
                let escaped: Vec<String> =
                    all_of.iter().map(|l| shell_escape(l)).collect();
                out.push_str("    ALL_LABELS_MATCH=true\n");
                for label in &escaped {
                    out.push_str(&format!(
                        "    if ! echo \"$PR_LABELS\" | grep -qiF '{}'; then\n",
                        label
                    ));
                    out.push_str("      ALL_LABELS_MATCH=false\n");
                    out.push_str("    fi\n");
                }
                out.push_str("    if [ \"$ALL_LABELS_MATCH\" = \"true\" ]; then\n");
                out.push_str("      echo \"Filter: labels all-of | Result: PASS\"\n");
                out.push_str("    else\n");
                out.push_str(&format!(
                    "      echo \"##[warning]Filter labels all-of did not match (required all of: {})\"\n",
                    escaped.join(", ")
                ));
                out.push_str(&format!(
                    "      echo \"##vso[build.addbuildtag]{}\"\n",
                    tag
                ));
                out.push_str("      SHOULD_RUN=false\n");
                out.push_str("    fi\n");
            }

            if !none_of.is_empty() {
                let escaped: Vec<String> =
                    none_of.iter().map(|l| shell_escape(l)).collect();
                out.push_str("    BLOCKED_LABEL_FOUND=false\n");
                for label in &escaped {
                    out.push_str(&format!(
                        "    if echo \"$PR_LABELS\" | grep -qiF '{}'; then\n",
                        label
                    ));
                    out.push_str("      BLOCKED_LABEL_FOUND=true\n");
                    out.push_str("    fi\n");
                }
                out.push_str("    if [ \"$BLOCKED_LABEL_FOUND\" = \"false\" ]; then\n");
                out.push_str("      echo \"Filter: labels none-of | Result: PASS\"\n");
                out.push_str("    else\n");
                out.push_str(&format!(
                    "      echo \"##[warning]Filter labels none-of matched a blocked label (blocked: {})\"\n",
                    escaped.join(", ")
                ));
                out.push_str(&format!(
                    "      echo \"##vso[build.addbuildtag]{}\"\n",
                    tag
                ));
                out.push_str("      SHOULD_RUN=false\n");
                out.push_str("    fi\n");
            }
        }

        Predicate::FileGlobMatch { include, exclude } => {
            let include_patterns: Vec<String> =
                include.iter().map(|p| format!("\"{}\"", shell_escape(p))).collect();
            let exclude_patterns: Vec<String> =
                exclude.iter().map(|p| format!("\"{}\"", shell_escape(p))).collect();
            let include_list = if include_patterns.is_empty() {
                "[]".to_string()
            } else {
                format!("[{}]", include_patterns.join(", "))
            };
            let exclude_list = if exclude_patterns.is_empty() {
                "[]".to_string()
            } else {
                format!("[{}]", exclude_patterns.join(", "))
            };

            out.push_str("    # Changed files filter\n");
            out.push_str(&format!(
                concat!(
                    "    FILES_MATCH=$(echo \"$CHANGED_FILES\" | python3 -c \"\n",
                    "import sys, fnmatch\n",
                    "includes = {}\n",
                    "excludes = {}\n",
                    "files = [l.strip() for l in sys.stdin if l.strip()]\n",
                    "matched = []\n",
                    "for f in files:\n",
                    "    inc = not includes or any(fnmatch.fnmatch(f, p) for p in includes)\n",
                    "    exc = any(fnmatch.fnmatch(f, p) for p in excludes)\n",
                    "    if inc and not exc:\n",
                    "        matched.append(f)\n",
                    "print('true' if matched else 'false')\n",
                    "\" 2>/dev/null || echo 'true')\n",
                ),
                include_list, exclude_list,
            ));
            out.push_str("    if [ \"$FILES_MATCH\" = \"true\" ]; then\n");
            out.push_str(
                "      echo \"Filter: changed-files | Result: PASS\"\n",
            );
            out.push_str("    else\n");
            out.push_str(
                "      echo \"##[warning]Filter changed-files did not match any relevant files\"\n",
            );
            out.push_str(&format!(
                "      echo \"##vso[build.addbuildtag]{}\"\n",
                tag
            ));
            out.push_str("      SHOULD_RUN=false\n");
            out.push_str("    fi\n");
        }

        // Logical combinators — these are internal and not expected at the
        // top level of a FilterCheck. If encountered, evaluate inline.
        Predicate::And(_) | Predicate::Or(_) | Predicate::Not(_) => {
            // Currently unused at top level. Reserved for future compound filters.
            out.push_str(&format!(
                "    # {} filter (compound — not yet implemented)\n",
                check.name
            ));
        }
    }
}

/// Capitalize the first letter of a string.
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().to_string() + chars.as_str(),
    }
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
    fn test_fact_shell_vars_are_unique() {
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
        let vars: BTreeSet<&str> =
            all_facts.iter().map(|f| f.shell_var()).collect();
        assert_eq!(vars.len(), all_facts.len(), "shell variable names must be unique");
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
                pattern: "\\[review\\]".into(),
            }),
            ..Default::default()
        };
        let checks = lower_pr_filters(&filters);
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].name, "title");
        assert!(matches!(
            &checks[0].predicate,
            Predicate::RegexMatch { fact: Fact::PrTitle, pattern } if pattern == "\\[review\\]"
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
                pattern: "\\[review\\]".into(),
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
        let result = compile_gate_step(GateContext::PullRequest, &[]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_compile_gate_step_pr_bypass() {
        let checks = vec![FilterCheck {
            name: "title",
            predicate: Predicate::RegexMatch {
                fact: Fact::PrTitle,
                pattern: "test".into(),
            },
            build_tag_suffix: "title-mismatch",
        }];
        let result = compile_gate_step(GateContext::PullRequest, &checks);
        assert!(result.contains("PullRequest"));
        assert!(result.contains("gate passes automatically"));
        assert!(result.contains("SHOULD_RUN"));
    }

    #[test]
    fn test_compile_gate_step_pipeline_bypass() {
        let checks = vec![FilterCheck {
            name: "source-pipeline",
            predicate: Predicate::RegexMatch {
                fact: Fact::TriggeredByPipeline,
                pattern: "Build.*".into(),
            },
            build_tag_suffix: "source-pipeline-mismatch",
        }];
        let result = compile_gate_step(GateContext::PipelineCompletion, &checks);
        assert!(result.contains("ResourceTrigger"));
        assert!(result.contains("pipeline-gate"));
        assert!(result.contains("pipelineGate"));
    }

    #[test]
    fn test_compile_gate_step_acquires_facts() {
        let checks = vec![
            FilterCheck {
                name: "title",
                predicate: Predicate::RegexMatch {
                    fact: Fact::PrTitle,
                    pattern: "test".into(),
                },
                build_tag_suffix: "title-mismatch",
            },
            FilterCheck {
                name: "draft",
                predicate: Predicate::Equality {
                    fact: Fact::PrIsDraft,
                    value: "false".into(),
                },
                build_tag_suffix: "draft-mismatch",
            },
        ];
        let result = compile_gate_step(GateContext::PullRequest, &checks);
        // Should acquire PrTitle and PrMetadata (dependency of PrIsDraft)
        assert!(
            result.contains("TITLE=\"$(System.PullRequest.Title)\""),
            "should acquire PrTitle"
        );
        assert!(
            result.contains("pullRequests"),
            "should acquire PrMetadata for draft check"
        );
        assert!(result.contains("isDraft"), "should acquire PrIsDraft");
    }

    #[test]
    fn test_compile_gate_step_self_cancel() {
        let checks = vec![FilterCheck {
            name: "title",
            predicate: Predicate::RegexMatch {
                fact: Fact::PrTitle,
                pattern: "test".into(),
            },
            build_tag_suffix: "title-mismatch",
        }];
        let result = compile_gate_step(GateContext::PullRequest, &checks);
        assert!(result.contains("cancelling"), "should include self-cancel");
        assert!(
            result.contains("SYSTEM_ACCESSTOKEN"),
            "should pass access token"
        );
    }

    #[test]
    fn test_compile_gate_step_labels() {
        let checks = vec![FilterCheck {
            name: "labels",
            predicate: Predicate::LabelSetMatch {
                any_of: vec!["run-agent".into()],
                all_of: vec![],
                none_of: vec!["do-not-run".into()],
            },
            build_tag_suffix: "labels-mismatch",
        }];
        let result = compile_gate_step(GateContext::PullRequest, &checks);
        assert!(result.contains("run-agent"), "should check for run-agent");
        assert!(result.contains("do-not-run"), "should check for blocked label");
        assert!(result.contains("LABEL_MATCH"), "should use any-of matching");
        assert!(
            result.contains("BLOCKED_LABEL_FOUND"),
            "should use none-of matching"
        );
    }

    #[test]
    fn test_compile_gate_step_changed_files() {
        let checks = vec![FilterCheck {
            name: "changed-files",
            predicate: Predicate::FileGlobMatch {
                include: vec!["src/**/*.rs".into()],
                exclude: vec!["docs/**".into()],
            },
            build_tag_suffix: "changed-files-mismatch",
        }];
        let result = compile_gate_step(GateContext::PullRequest, &checks);
        assert!(result.contains("iterations"), "should fetch iteration changes");
        assert!(result.contains("fnmatch"), "should use fnmatch");
        assert!(result.contains("src/**/*.rs"), "should include pattern");
    }

    #[test]
    fn test_compile_gate_step_time_window() {
        let checks = vec![FilterCheck {
            name: "time-window",
            predicate: Predicate::TimeWindow {
                start: "09:00".into(),
                end: "17:00".into(),
            },
            build_tag_suffix: "time-window-mismatch",
        }];
        let result = compile_gate_step(GateContext::PullRequest, &checks);
        assert!(result.contains("CURRENT_HOUR"), "should get current UTC hour");
        assert!(result.contains("09:00"), "should include start time");
        assert!(result.contains("17:00"), "should include end time");
        assert!(result.contains("IN_WINDOW"), "should evaluate time window");
    }

    #[test]
    fn test_compile_gate_step_numeric_range() {
        let checks = vec![FilterCheck {
            name: "change-count",
            predicate: Predicate::NumericRange {
                fact: Fact::ChangedFileCount,
                min: Some(5),
                max: Some(100),
            },
            build_tag_suffix: "changes-mismatch",
        }];
        let result = compile_gate_step(GateContext::PullRequest, &checks);
        assert!(result.contains("-ge 5"), "should check min");
        assert!(result.contains("-le 100"), "should check max");
    }

    // ─── End-to-end lowering + codegen ──────────────────────────────────

    #[test]
    fn test_roundtrip_pr_filters_to_bash() {
        let filters = PrFilters {
            title: Some(PatternFilter {
                pattern: "\\[review\\]".into(),
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

        let bash = compile_gate_step(GateContext::PullRequest, &checks);
        assert!(bash.contains("System.PullRequest.Title"));
        assert!(bash.contains("isDraft"));
        assert!(bash.contains("run-agent"));
        assert!(bash.contains("prGate"));
    }
}
