//! [`Job`] — a single ADO job inside a stage (or directly under the
//! top-level `jobs:` key for un-staged pipelines).
//!
//! `depends_on` is **derived**, not user-supplied: the
//! `ir-graph` commit walks every [`super::output::OutputRef`] /
//! [`super::condition::Condition`] inside the job's steps and adds an
//! edge for each producer that lives in a different job.

use std::time::Duration;

use super::condition::Condition;
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
    },
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
    fn pool_variants_are_distinct() {
        let a = Pool::VmImage("ubuntu-22.04".into());
        let b = Pool::Named {
            name: "AZS-1ES-L".into(),
            image: None,
            os: None,
        };
        assert_ne!(a, b);
    }

    #[test]
    fn new_starts_empty_depends_on_and_steps() {
        let j = Job::new(
            JobId::new("Agent").unwrap(),
            "Agent",
            Pool::VmImage("ubuntu-22.04".into()),
        );
        assert!(j.depends_on.is_empty());
        assert!(j.steps.is_empty());
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
            persist_credentials: None,
        }));
        assert_eq!(j.steps.len(), 1);
    }
}
