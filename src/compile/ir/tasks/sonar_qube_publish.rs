//! Typed builder for `SonarQubePublish@8`.

use crate::compile::ir::step::TaskStep;

/// Builder for a [`TaskStep`] invoking `SonarQubePublish@8`.
///
/// Publishes SonarQube analysis results and waits for the Quality Gate
/// outcome. This is the third and final step in the SonarQube pipeline
/// trio: `SonarQubePrepare` → `SonarQubeAnalyze` → `SonarQubePublish`.
///
/// The task polls the SonarQube server until the analysis is complete and
/// then surfaces the Quality Gate status (pass/fail) as a pipeline outcome.
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/sonar-qube-publish-v8-task>
#[derive(Debug, Clone)]
pub struct SonarQubePublish {
    polling_timeout_sec: Option<u32>,
    display_name: Option<String>,
}

impl SonarQubePublish {
    /// Create a new `SonarQubePublish@8` builder.
    ///
    /// The only ADO input (`pollingTimeoutSec`) has a default of `300`; call
    /// [`polling_timeout_sec`](SonarQubePublish::polling_timeout_sec) to
    /// override it.
    pub fn new() -> Self {
        Self {
            polling_timeout_sec: None,
            display_name: None,
        }
    }

    /// `pollingTimeoutSec` — maximum seconds to wait for the Quality Gate
    /// result. ADO default is `300`.
    pub fn polling_timeout_sec(mut self, secs: u32) -> Self {
        self.polling_timeout_sec = Some(secs);
        self
    }

    /// Override the default `displayName` (`"Publish Quality Gate Result"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "SonarQubePublish@8",
            self.display_name
                .unwrap_or_else(|| "Publish Quality Gate Result".into()),
        );
        if let Some(secs) = self.polling_timeout_sec {
            t = t.with_input("pollingTimeoutSec", secs.to_string());
        }
        t
    }
}

impl Default for SonarQubePublish {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_emits_task_with_no_inputs() {
        let t = SonarQubePublish::new().into_step();
        assert_eq!(t.task, "SonarQubePublish@8");
        assert_eq!(t.display_name, "Publish Quality Gate Result");
        assert!(t.inputs.is_empty(), "no inputs should be emitted when using ADO defaults");
    }

    #[test]
    fn polling_timeout_emitted_when_set() {
        let t = SonarQubePublish::new().polling_timeout_sec(600).into_step();
        assert_eq!(
            t.inputs.get("pollingTimeoutSec").map(String::as_str),
            Some("600")
        );
    }

    #[test]
    fn polling_timeout_not_emitted_when_unset() {
        let t = SonarQubePublish::new().into_step();
        assert!(t.inputs.get("pollingTimeoutSec").is_none());
    }

    #[test]
    fn display_name_override() {
        let t = SonarQubePublish::new()
            .with_display_name("Wait for Quality Gate")
            .into_step();
        assert_eq!(t.display_name, "Wait for Quality Gate");
    }

    #[test]
    fn default_trait_matches_new() {
        let t1 = SonarQubePublish::new().into_step();
        let t2 = SonarQubePublish::default().into_step();
        assert_eq!(t1.task, t2.task);
        assert_eq!(t1.display_name, t2.display_name);
    }
}
