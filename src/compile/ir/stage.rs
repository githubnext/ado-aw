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
}

impl Stage {
    pub fn new(id: StageId, display_name: impl Into<String>) -> Self {
        Self {
            id,
            display_name: display_name.into(),
            jobs: Vec::new(),
            depends_on: Vec::new(),
            condition: None,
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
}
