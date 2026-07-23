//! [`Job`] — a single ADO job inside a stage (or directly under the
//! top-level `jobs:` key for un-staged pipelines).
//!
//! `depends_on` is **derived**, not user-supplied: the
//! `ir-graph` commit walks every [`super::output::OutputRef`] /
//! [`super::condition::Condition`] inside the job's steps and adds an
//! edge for each producer that lives in a different job.

use std::time::Duration;

use anyhow::Result;

use super::condition::Condition;
use super::env::EnvValue;
use super::ids::JobId;
use super::step::Step;

/// A single ADO job.
#[derive(Debug, Clone)]
pub struct Job {
    pub id: JobId,
    pub display_name: String,
    pub pool: Pool,
    pub timeout: Option<Duration>,
    pub steps: Vec<Step>,
    /// **Derived** by the graph pass — extension authors should not
    /// populate this directly. The graph pass treats a non-empty
    /// value as a manual override.
    pub depends_on: Vec<JobId>,
    pub condition: Option<Condition>,
    /// Job-level `variables:` block. ADO's documented safe location
    /// for cross-job step-output references (`dependencies.<Job>.outputs[...]`)
    /// — step-level `env:` does not reliably evaluate those runtime
    /// expressions (see PR #956 — empirically verified against
    /// msazuresphere/4x4 build #612290 / #612528). Step env then
    /// reads the hoisted value via the same-job `$(name)` macro.
    pub variables: Vec<JobVariable>,
    /// When set, the lowering pass emits dual-branch
    /// `${{ if eq(length(parameters.<X>), 0) }}` /
    /// `${{ if ne(length(parameters.<X>), 0) }}` blocks for both
    /// `dependsOn:` and `condition:` so callers can pass external
    /// values at the template-invocation site that merge with the
    /// job's internal `depends_on` / `condition`. Used by the Agent
    /// job in `target: job`.
    ///
    /// The internal `depends_on` list is emitted as the "caller-
    /// omitted" branch and prefixed onto the caller-supplied list in
    /// the "caller-provided" branch. The internal `condition` is
    /// emitted as the "caller-omitted" branch body; the caller's
    /// condition is appended into the same `and(…)` body in the
    /// "caller-provided" branch.
    pub template_dependson_wrap: Option<TemplateDependsOnWrap>,
    /// 1ES `templateContext:` wrap. When `Some`, the lowering pass:
    ///
    /// - Suppresses the per-job `pool:` key (1ES jobs inherit the
    ///   pool from `extends.parameters.pool`).
    /// - Wraps the job's `steps:` under a `templateContext:` block:
    ///   ```yaml
    ///   templateContext:
    ///     type: buildJob
    ///     outputs: …    # collected from Step::Publish entries
    ///     steps: …      # remaining steps (publishes filtered out)
    ///   ```
    /// - Collects every `Step::Publish` in the job's `steps:` list
    ///   into a `templateContext.outputs[]` entry of shape
    ///   `{ output: pipelineArtifact, path: …, artifact: …,
    ///   condition: always() }`. The `Step::Publish` entries are
    ///   *removed* from the emitted `steps:` so the artifact is
    ///   published once (by the 1ES template machinery), not twice.
    ///
    /// `None` (the default) preserves today's standalone behaviour:
    /// per-job `pool:` is emitted and `Step::Publish` lowers as an
    /// inline step.
    pub template_context: Option<JobTemplateContext>,
}

/// Per-job `templateContext:` configuration. See
/// [`Job::template_context`].
#[derive(Debug, Clone)]
pub struct JobTemplateContext {
    /// `type:` field. Today only `"buildJob"` is used.
    pub kind: TemplateContextKind,
}

impl Default for JobTemplateContext {
    fn default() -> Self {
        Self {
            kind: TemplateContextKind::BuildJob,
        }
    }
}

/// `templateContext.type:` enumeration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateContextKind {
    /// `type: buildJob` — the standard 1ES build-job template.
    BuildJob,
}

impl TemplateContextKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            TemplateContextKind::BuildJob => "buildJob",
        }
    }
}

/// A single Agent-job-level `variables:` entry. The `value` is a
/// typed [`EnvValue`] so cross-job [`super::output::OutputRef`]
/// references in the value lower to the correct ADO reference
/// syntax (`$[ coalesce(dependencies.<Job>.outputs['<step>.<X>'], '') ]`
/// for cross-job consumers in the same stage).
#[derive(Debug, Clone)]
pub struct JobVariable {
    pub name: String,
    pub value: EnvValue,
}

/// Template-parameter wrap for a [`Job`]. See
/// [`Job::template_dependson_wrap`].
#[derive(Debug, Clone)]
pub struct TemplateDependsOnWrap {
    /// Name of the template parameter carrying the external
    /// `dependsOn` value (always `"dependsOn"` today). MUST be a valid
    /// ADO parameter identifier (`[A-Za-z_][A-Za-z0-9_]*`).
    pub depends_on_param: String,
    /// Name of the template parameter carrying the external
    /// `condition` value (always `"condition"` today). MUST be a valid
    /// ADO parameter identifier (`[A-Za-z_][A-Za-z0-9_]*`).
    pub condition_param: String,
}

impl TemplateDependsOnWrap {
    /// Construct a template-parameter wrap, validating the parameter
    /// names before they are embedded into ADO template-expression
    /// YAML keys.
    pub fn new(
        depends_on_param: impl Into<String>,
        condition_param: impl Into<String>,
    ) -> Result<Self> {
        let depends_on_param = depends_on_param.into();
        let condition_param = condition_param.into();
        validate_template_parameter_name(
            "TemplateDependsOnWrap::depends_on_param",
            &depends_on_param,
        )?;
        validate_template_parameter_name(
            "TemplateDependsOnWrap::condition_param",
            &condition_param,
        )?;
        Ok(Self {
            depends_on_param,
            condition_param,
        })
    }
}

fn validate_template_parameter_name(field: &str, value: &str) -> Result<()> {
    if !crate::validate::is_valid_parameter_name(value) {
        anyhow::bail!(
            "{field} must be a valid ADO parameter identifier \
             ([A-Za-z_][A-Za-z0-9_]*); got {value:?}"
        );
    }
    Ok(())
}

/// ADO job pool. Captures the two shapes ado-aw uses today
/// (`pool: { vmImage: … }` and `pool: { name: … }`); extends with
/// host attributes (image / os) when 1ES needs them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pool {
    /// `vmImage: <image>` — Microsoft-hosted agents.
    VmImage(String),
    /// `name: <pool_name>` — self-hosted agent pool.
    Named {
        name: String,
        /// Optional `image:` field (1ES pool images).
        image: Option<String>,
        /// Optional `os:` field (1ES pool OS).
        os: Option<String>,
        /// Ordered Azure Pipelines demands for self-hosted pools.
        demands: Vec<String>,
    },
    /// `server` — an agentless (server) job. Emits the scalar
    /// `pool: server`. Required for server-only tasks such as
    /// `ManualValidation@1`; such jobs must contain no agent steps
    /// (no checkout, downloads, or shell/`ado-aw` invocations).
    Server,
}

impl Job {
    /// Construct a minimal job — caller fills `steps` and any
    /// optional fields.
    pub fn new(id: JobId, display_name: impl Into<String>, pool: Pool) -> Self {
        Self {
            id,
            display_name: display_name.into(),
            pool,
            timeout: None,
            steps: Vec::new(),
            depends_on: Vec::new(),
            condition: None,
            variables: Vec::new(),
            template_dependson_wrap: None,
            template_context: None,
        }
    }

    /// Append a step.
    pub fn push_step(&mut self, step: Step) {
        self.steps.push(step);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_same_variant_different_values_are_not_equal() {
        // Equality semantics for same-variant: fields must match.
        let a = Pool::VmImage("ubuntu-22.04".into());
        let b = Pool::VmImage("windows-2022".into());
        assert_ne!(a, b, "different vmImage values should not be equal");

        let c = Pool::Named {
            name: "Pool-A".into(),
            image: None,
            os: None,
            demands: Vec::new(),
        };
        let d = Pool::Named {
            name: "Pool-B".into(),
            image: None,
            os: None,
            demands: Vec::new(),
        };
        assert_ne!(c, d, "different pool names should not be equal");

        // Same values → equal.
        let e = Pool::VmImage("ubuntu-22.04".into());
        assert_eq!(a, e, "identical VmImage values should be equal");
    }

    #[test]
    fn new_starts_empty_depends_on_and_steps() {
        let j = Job::new(
            JobId::new("Agent").unwrap(),
            "Agent",
            Pool::VmImage("ubuntu-22.04".into()),
        );
        assert!(j.depends_on.is_empty(), "depends_on should start empty");
        assert!(j.steps.is_empty(), "steps should start empty");
        assert!(j.condition.is_none(), "condition should start as None");
        assert!(j.variables.is_empty(), "variables should start empty");
        assert!(
            j.template_dependson_wrap.is_none(),
            "template_dependson_wrap should start as None"
        );
        assert!(
            j.template_context.is_none(),
            "template_context should start as None"
        );
    }

    #[test]
    fn push_step_appends() {
        let mut j = Job::new(
            JobId::new("Setup").unwrap(),
            "Setup",
            Pool::VmImage("ubuntu-22.04".into()),
        );
        j.push_step(Step::Checkout(super::super::step::CheckoutStep {
            repository: super::super::step::CheckoutRepo::Self_,
            clean: None,
            submodules: None,
            fetch_depth: None,
            fetch_tags: None,
            persist_credentials: None,
            path: None,
        }));
        assert_eq!(j.steps.len(), 1);
    }

    #[test]
    fn template_depends_on_wrap_rejects_invalid_parameter_names() {
        let err = TemplateDependsOnWrap::new("dependsOn", "bad }}").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("TemplateDependsOnWrap::condition_param"));
        assert!(msg.contains("valid ADO parameter identifier"));
    }
}
