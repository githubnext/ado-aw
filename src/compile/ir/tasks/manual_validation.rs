//! Typed builder for `ManualValidation@1`.
//!
//! ADO task reference:
//! <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/manual-validation-v1>

use super::common::{de_opt_bool_flex, push_bool, push_opt};
use crate::compile::ir::step::TaskStep;
use serde::Deserialize;

/// What the task should do when the timeout elapses with no human response.
///
/// Maps to the `onTimeout` input of `ManualValidation@1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum OnTimeout {
    /// Automatically reject the run on timeout (ADO default).
    #[serde(rename = "reject")]
    Reject,
    /// Automatically resume (approve) the run on timeout.
    #[serde(rename = "resume")]
    Resume,
}

impl OnTimeout {
    /// Returns the exact ADO token for this value.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            Self::Reject => "reject",
            Self::Resume => "resume",
        }
    }
}

/// Builder for a [`TaskStep`] invoking `ManualValidation@1`.
///
/// Pauses a pipeline run within a stage to allow a human to review, approve,
/// or reject before execution continues. Useful for human-in-the-loop agentic
/// workflows that require oversight before a consequential action is taken.
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/manual-validation-v1>
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManualValidation {
    #[serde(rename = "notifyUsers")]
    notify_users: String,
    #[serde(rename = "approvers", default)]
    approvers: Option<String>,
    #[serde(
        rename = "allowApproversToApproveTheirOwnRuns",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    allow_approvers_to_approve_their_own_runs: Option<bool>,
    #[serde(rename = "instructions", default)]
    instructions: Option<String>,
    #[serde(rename = "onTimeout", default)]
    on_timeout: Option<OnTimeout>,
    #[serde(skip)]
    timeout_minutes: Option<u32>,
    #[serde(skip)]
    display_name: Option<String>,
}

impl ManualValidation {
    /// Create a new builder.
    ///
    /// `notify_users` — required by ADO but may be an empty string if you do
    /// not want to send notification emails. Accepts a comma-separated list of
    /// users and/or groups, e.g. `"user@example.com,  [org]\\team"`.
    pub fn new(notify_users: impl Into<String>) -> Self {
        Self {
            notify_users: notify_users.into(),
            approvers: None,
            allow_approvers_to_approve_their_own_runs: None,
            instructions: None,
            on_timeout: None,
            timeout_minutes: None,
            display_name: None,
        }
    }

    /// `approvers` — comma-separated list of users/groups/project-teams
    /// permitted to act on this validation. When absent, anyone with *Queue
    /// build* permission may approve or reject.
    pub fn approvers(mut self, value: impl Into<String>) -> Self {
        self.approvers = Some(value.into());
        self
    }

    /// `allowApproversToApproveTheirOwnRuns` — whether the user who triggered
    /// the build is allowed to approve their own run. ADO default: `true`.
    pub fn allow_approvers_to_approve_their_own_runs(mut self, value: bool) -> Self {
        self.allow_approvers_to_approve_their_own_runs = Some(value);
        self
    }

    /// `instructions` — free-text shown to the reviewer when the pipeline
    /// pauses, describing what manual steps are needed before approval.
    pub fn instructions(mut self, value: impl Into<String>) -> Self {
        self.instructions = Some(value.into());
        self
    }

    /// `onTimeout` — automatic response when the pending period expires
    /// (default: [`OnTimeout::Reject`]).
    pub fn on_timeout(mut self, value: OnTimeout) -> Self {
        self.on_timeout = Some(value);
        self
    }

    /// Bound the validation's pending period via the **step-level**
    /// `timeoutInMinutes` control option (NOT a task input — `ManualValidation@1`
    /// has none). This is the timeout that triggers the task's `onTimeout`
    /// handler: when it elapses the task applies `reject`/`resume` and completes
    /// gracefully. A *job*-level timeout, by contrast, cancels the job and never
    /// lets the task apply `onTimeout: resume`. `0`/`None` inherits the
    /// pipeline default.
    pub fn timeout_minutes(mut self, minutes: u32) -> Self {
        self.timeout_minutes = Some(minutes);
        self
    }

    /// Override the default `displayName` (`"Manual Validation"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "ManualValidation@1",
            self.display_name
                .unwrap_or_else(|| "Manual Validation".into()),
        )
        .with_input("notifyUsers", self.notify_users);
        push_opt(&mut t, "approvers", self.approvers);
        push_bool(
            &mut t,
            "allowApproversToApproveTheirOwnRuns",
            self.allow_approvers_to_approve_their_own_runs,
        );
        push_opt(&mut t, "instructions", self.instructions);
        if let Some(v) = self.on_timeout {
            t = t.with_input("onTimeout", v.as_ado_str());
        }
        if let Some(mins) = self.timeout_minutes {
            // Step-level `timeoutInMinutes` — the bound that fires the task's
            // `onTimeout` handler (see `timeout_minutes`). Lowered by the IR.
            t.timeout = Some(std::time::Duration::from_secs(60 * (mins as u64)));
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_task_and_required_input() {
        let t = ManualValidation::new("reviewer@example.com").into_step();
        assert_eq!(t.task, "ManualValidation@1");
        assert_eq!(t.display_name, "Manual Validation");
        assert_eq!(
            t.inputs.get("notifyUsers").map(String::as_str),
            Some("reviewer@example.com")
        );
    }

    #[test]
    fn empty_notify_users_is_valid() {
        let t = ManualValidation::new("").into_step();
        assert_eq!(t.inputs.get("notifyUsers").map(String::as_str), Some(""));
    }

    #[test]
    fn optional_inputs_not_emitted_by_default() {
        let t = ManualValidation::new("team@example.com").into_step();
        assert!(t.inputs.get("approvers").is_none());
        assert!(t
            .inputs
            .get("allowApproversToApproveTheirOwnRuns")
            .is_none());
        assert!(t.inputs.get("instructions").is_none());
        assert!(t.inputs.get("onTimeout").is_none());
    }

    #[test]
    fn approvers_input() {
        let t = ManualValidation::new("")
            .approvers("[MyOrg]\\release-approvers")
            .into_step();
        assert_eq!(
            t.inputs.get("approvers").map(String::as_str),
            Some("[MyOrg]\\release-approvers")
        );
    }

    #[test]
    fn allow_approvers_own_runs_false() {
        let t = ManualValidation::new("")
            .allow_approvers_to_approve_their_own_runs(false)
            .into_step();
        assert_eq!(
            t.inputs
                .get("allowApproversToApproveTheirOwnRuns")
                .map(String::as_str),
            Some("false")
        );
    }

    #[test]
    fn allow_approvers_own_runs_true() {
        let t = ManualValidation::new("")
            .allow_approvers_to_approve_their_own_runs(true)
            .into_step();
        assert_eq!(
            t.inputs
                .get("allowApproversToApproveTheirOwnRuns")
                .map(String::as_str),
            Some("true")
        );
    }

    #[test]
    fn instructions_input() {
        let t = ManualValidation::new("")
            .instructions("Please verify the staging environment before proceeding.")
            .into_step();
        assert_eq!(
            t.inputs.get("instructions").map(String::as_str),
            Some("Please verify the staging environment before proceeding.")
        );
    }

    #[test]
    fn on_timeout_reject() {
        let t = ManualValidation::new("")
            .on_timeout(OnTimeout::Reject)
            .into_step();
        assert_eq!(
            t.inputs.get("onTimeout").map(String::as_str),
            Some("reject")
        );
    }

    #[test]
    fn timeout_minutes_sets_step_timeout_not_an_input() {
        let t = ManualValidation::new("").timeout_minutes(120).into_step();
        // It is a step-level control option (lowers to timeoutInMinutes), not
        // a task input — ManualValidation@1 has no `timeout` input.
        assert!(t.inputs.get("timeout").is_none());
        assert_eq!(t.timeout, Some(std::time::Duration::from_secs(120 * 60)));
    }

    #[test]
    fn no_timeout_minutes_leaves_step_timeout_unset() {
        let t = ManualValidation::new("").into_step();
        assert!(t.timeout.is_none());
    }

    #[test]
    fn on_timeout_resume() {
        let t = ManualValidation::new("")
            .on_timeout(OnTimeout::Resume)
            .into_step();
        assert_eq!(
            t.inputs.get("onTimeout").map(String::as_str),
            Some("resume")
        );
    }

    #[test]
    fn display_name_override() {
        let t = ManualValidation::new("ops@example.com")
            .with_display_name("Await release approval")
            .into_step();
        assert_eq!(t.display_name, "Await release approval");
    }

    #[test]
    fn all_inputs() {
        let t = ManualValidation::new("ops@example.com, [Org]\\infra")
            .approvers("[Org]\\release-team")
            .allow_approvers_to_approve_their_own_runs(false)
            .instructions("Verify deployment health before continuing.")
            .on_timeout(OnTimeout::Resume)
            .with_display_name("Production gate: manual approval")
            .into_step();
        assert_eq!(t.task, "ManualValidation@1");
        assert_eq!(t.display_name, "Production gate: manual approval");
        assert_eq!(
            t.inputs.get("notifyUsers").map(String::as_str),
            Some("ops@example.com, [Org]\\infra")
        );
        assert_eq!(
            t.inputs.get("approvers").map(String::as_str),
            Some("[Org]\\release-team")
        );
        assert_eq!(
            t.inputs
                .get("allowApproversToApproveTheirOwnRuns")
                .map(String::as_str),
            Some("false")
        );
        assert_eq!(
            t.inputs.get("instructions").map(String::as_str),
            Some("Verify deployment health before continuing.")
        );
        assert_eq!(
            t.inputs.get("onTimeout").map(String::as_str),
            Some("resume")
        );
    }

    #[test]
    fn on_timeout_as_ado_str() {
        assert_eq!(OnTimeout::Reject.as_ado_str(), "reject");
        assert_eq!(OnTimeout::Resume.as_ado_str(), "resume");
    }
}
