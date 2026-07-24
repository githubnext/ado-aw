//! Emit a [`Pipeline`] as a YAML string.
//!
//! The emit pass is intentionally thin: it composes the lowering
//! pass ([`super::lower::lower`]) with `serde_yaml::to_string`. The
//! resulting string is structurally identical (up to YAML
//! whitespace) to the canonical-form pipelines that the prep PR
//! (commit `f8aab33a`) established as the formatting baseline.
//!
//! Callers should prepend the `# @ado-aw …` header comment via
//! [`crate::compile::common::generate_header_comment`] after this
//! function returns; the IR itself never embeds comments.

use anyhow::{Context, Result};

use super::Pipeline;

/// Lower a [`Pipeline`] to YAML.
pub fn emit(pipeline: &Pipeline) -> Result<String> {
    let value = super::lower::lower(pipeline).context("ir::emit: lowering failed")?;
    serde_yaml::to_string(&value).context("ir::emit: serde_yaml serialisation failed")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::ir::ids::{JobId, StageId};
    use crate::compile::ir::job::{Job, Pool};
    use crate::compile::ir::stage::Stage;
    use crate::compile::ir::step::{
        BashStep, CheckoutRepo, CheckoutStep, DownloadStep, PublishStep, Step,
    };
    use crate::compile::ir::{PipelineBody, PipelineShape, Resources, Triggers};
    use serde_yaml::Value;

    fn pipeline_with_jobs(jobs: Vec<Job>) -> Pipeline {
        Pipeline {
            name: "Test-$(BuildID)".into(),
            parameters: Vec::new(),
            resources: Resources::default(),
            triggers: Triggers::default(),
            variables: Vec::new(),
            body: PipelineBody::Jobs(jobs),
            shape: PipelineShape::Standalone,
        }
    }

    fn pool() -> Pool {
        Pool::VmImage("ubuntu-22.04".into())
    }

    /// The load-bearing acceptance test for this commit:
    /// `IR → emit → serde_yaml::from_str` round-trips to the same
    /// `serde_yaml::Value` we would build by hand from the same IR.
    #[test]
    fn emit_round_trips_standalone_pipeline_to_equal_value() {
        let mut setup = Job::new(JobId::new("Setup").unwrap(), "Setup", pool());
        setup.push_step(Step::Checkout(CheckoutStep {
            repository: CheckoutRepo::Self_,
            clean: Some(true),
            submodules: None,
            fetch_depth: None,
            fetch_tags: None,
            persist_credentials: None,
            path: None,
        }));
        setup.push_step(Step::Bash(BashStep::new("Prep", "echo prep")));

        let mut agent = Job::new(JobId::new("Agent").unwrap(), "Agent", pool());
        agent.push_step(Step::Bash(BashStep::new("Run", "echo run")));
        agent.push_step(Step::Publish(PublishStep {
            path: "$(Agent.TempDirectory)/out".into(),
            artifact: "out".into(),
            condition: None,
        }));

        let pipeline = pipeline_with_jobs(vec![setup, agent]);

        let yaml = emit(&pipeline).unwrap();
        let reparsed: Value =
            serde_yaml::from_str(&yaml).expect("emit output must be parseable YAML");

        // Build the same tree by hand and compare structurally.
        // Mapping equality in serde_yaml is key-set + value equality —
        // insertion order does NOT affect the comparison, so this test
        // is robust to future reordering as long as it remains
        // semantically equivalent.
        let expected: Value = serde_yaml::from_str(
            r#"
name: "Test-$(BuildID)"
jobs:
  - job: Setup
    displayName: "Setup"
    pool: { vmImage: "ubuntu-22.04" }
    steps:
      - checkout: self
        clean: true
      - bash: "echo prep"
        displayName: "Prep"
  - job: Agent
    displayName: "Agent"
    pool: { vmImage: "ubuntu-22.04" }
    steps:
      - bash: "echo run"
        displayName: "Run"
      - publish: "$(Agent.TempDirectory)/out"
        artifact: "out"
"#,
        )
        .unwrap();

        assert_eq!(reparsed, expected, "emit output: {yaml}");
    }

    #[test]
    fn emit_round_trips_staged_pipeline_to_equal_value() {
        let mut agent = Job::new(JobId::new("Agent").unwrap(), "Agent", pool());
        agent.push_step(Step::Bash(BashStep::new("Run", "echo run")));

        let mut stage = Stage::new(StageId::new("Main").unwrap(), "Main");
        stage.push_job(agent);

        let pipeline = Pipeline {
            name: "Staged-$(BuildID)".into(),
            parameters: Vec::new(),
            resources: Resources::default(),
            triggers: Triggers::default(),
            variables: Vec::new(),
            body: PipelineBody::Stages(vec![stage]),
            shape: PipelineShape::Standalone,
        };

        let yaml = emit(&pipeline).unwrap();
        let reparsed: Value = serde_yaml::from_str(&yaml).expect("parseable");
        let expected: Value = serde_yaml::from_str(
            r#"
name: "Staged-$(BuildID)"
stages:
  - stage: Main
    displayName: "Main"
    jobs:
      - job: Agent
        displayName: "Agent"
        pool: { vmImage: "ubuntu-22.04" }
        steps:
          - bash: "echo run"
            displayName: "Run"
"#,
        )
        .unwrap();

        assert_eq!(reparsed, expected, "emit output: {yaml}");
    }

    #[test]
    fn emit_round_trips_download_step() {
        // DownloadStep has its own emit path (no nested env/condition
        // builder) so a dedicated round-trip catches accidental
        // wire-shape drift.
        let mut agent = Job::new(JobId::new("Agent").unwrap(), "Agent", pool());
        agent.push_step(Step::Download(DownloadStep {
            source: "current".into(),
            artifact: "agent_outputs_$(Build.BuildId)".into(),
            condition: None,
        }));

        let pipeline = pipeline_with_jobs(vec![agent]);
        let yaml = emit(&pipeline).unwrap();
        let reparsed: Value = serde_yaml::from_str(&yaml).unwrap();
        let download = &reparsed["jobs"][0]["steps"][0];
        assert_eq!(download["download"].as_str(), Some("current"));
        assert_eq!(
            download["artifact"].as_str(),
            Some("agent_outputs_$(Build.BuildId)")
        );
    }
}
