//! [`Stage`] — a group of jobs inside an ADO stages-pipeline. Used
//! by `OneEs` (which wraps everything in a single stage inside an
//! `extends:` template) and `StageTemplate` (the `target: stage`
//! compiler).
//!
//! Standalone and `target: job` pipelines emit a flat top-level
//! `jobs:` block and skip [`Stage`] altogether; see
//! [`super::PipelineBody::Jobs`].

use super::condition::Condition;
use super::ids::StageId;
use super::job::Job;

/// A single ADO stage.
#[derive(Debug, Clone)]
pub struct Stage {
    pub id: StageId,
    pub display_name: String,
    pub jobs: Vec<Job>,
    /// **Derived** by the graph pass from the cross-stage edges of
    /// the contained jobs' [`super::output::OutputRef`]s.
    pub depends_on: Vec<StageId>,
    pub condition: Option<Condition>,
    /// When set, the lowering pass emits caller-facing
    /// `${{ if ne(length(parameters.<name>), 0) }}: dependsOn:` and
    /// `${{ if ne(parameters.<name>, '') }}: condition:` blocks
    /// instead of the typed `dependsOn:` / `condition:` keys. Used
    /// by `target: stage` so callers can pass stage ordering at the
    /// template-invocation site (ADO disallows `dependsOn:` /
    /// `condition:` as bare keys at a `- template:` call site — only
    /// `template:` and `parameters:` are valid per the
    /// `stages.template` schema).
    ///
    /// When `external_params_wrap` is `Some`, the typed
    /// `depends_on` / `condition` fields should be empty (the wrap
    /// expects an empty internal stage; this is enforced by the
    /// validation pass).
    pub external_params_wrap: Option<StageExternalParamsWrap>,
}

/// External-parameter wrap for a [`Stage`]. See
/// [`Stage::external_params_wrap`].
#[derive(Debug, Clone)]
pub struct StageExternalParamsWrap {
    /// Name of the template parameter carrying the external
    /// `dependsOn` value (always `"dependsOn"` today). MUST be a valid
    /// ADO parameter identifier (`[A-Za-z_][A-Za-z0-9_]*`).
    pub depends_on_param: String,
    /// Name of the template parameter carrying the external
    /// `condition` value (always `"condition"` today). MUST be a valid
    /// ADO parameter identifier (`[A-Za-z_][A-Za-z0-9_]*`).
    pub condition_param: String,
}

impl StageExternalParamsWrap {
    /// Construct an external-parameter wrap, validating the parameter
    /// names before they are embedded into ADO template-expression
    /// YAML keys.
    pub fn new(
        depends_on_param: impl Into<String>,
        condition_param: impl Into<String>,
    ) -> anyhow::Result<Self> {
        let depends_on_param = depends_on_param.into();
        let condition_param = condition_param.into();
        validate_template_parameter_name(
            "StageExternalParamsWrap::depends_on_param",
            &depends_on_param,
        )?;
        validate_template_parameter_name(
            "StageExternalParamsWrap::condition_param",
            &condition_param,
        )?;
        Ok(Self {
            depends_on_param,
            condition_param,
        })
    }
}

fn validate_template_parameter_name(field: &str, value: &str) -> anyhow::Result<()> {
    if !crate::validate::is_valid_parameter_name(value) {
        anyhow::bail!(
            "{field} must be a valid ADO parameter identifier \
             ([A-Za-z_][A-Za-z0-9_]*); got {value:?}"
        );
    }
    Ok(())
}

impl Stage {
    pub fn new(id: StageId, display_name: impl Into<String>) -> Self {
        Self {
            id,
            display_name: display_name.into(),
            jobs: Vec::new(),
            depends_on: Vec::new(),
            condition: None,
            external_params_wrap: None,
        }
    }

    pub fn push_job(&mut self, job: Job) {
        self.jobs.push(job);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::ir::ids::JobId;
    use crate::compile::ir::job::Pool;

    #[test]
    fn new_starts_empty_jobs_and_depends_on() {
        let s = Stage::new(StageId::new("Main").unwrap(), "Main");
        assert!(s.jobs.is_empty());
        assert!(s.depends_on.is_empty());
        assert!(s.condition.is_none());
        assert!(s.external_params_wrap.is_none());
    }

    #[test]
    fn push_job_appends() {
        let mut s = Stage::new(StageId::new("Main").unwrap(), "Main");
        s.push_job(Job::new(
            JobId::new("Agent").unwrap(),
            "Agent",
            Pool::VmImage("ubuntu-22.04".into()),
        ));
        assert_eq!(s.jobs.len(), 1);
    }

    #[test]
    fn stage_external_params_wrap_rejects_invalid_parameter_names() {
        let err = StageExternalParamsWrap::new("bad name", "condition").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("StageExternalParamsWrap::depends_on_param"));
        assert!(msg.contains("valid ADO parameter identifier"));
    }
}
