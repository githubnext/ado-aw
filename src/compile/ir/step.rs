//! Individual pipeline steps.
//!
//! Each variant of [`Step`] corresponds to one of the ADO step shapes
//! we actually use today. Adding a new shape is a question of (a)
//! adding the variant, (b) extending [`Step::id`], and (c) wiring the
//! lowering pass.
//!
//! `BashStep::script` is the **raw bash body** — no leading
//! `- bash: |`. The YAML emit pass handles wrapping it in a literal
//! block scalar and indenting it correctly.
//!
//! Only the **types** are defined in the `ir-types` commit. The
//! step graph (`ir-graph`), output-ref lowering (`ir-output-lowering`),
//! and YAML emit (`ir-yaml-emit`) live in subsequent commits.

use indexmap::IndexMap;
use std::time::Duration;

use super::condition::Condition;
use super::env::EnvValue;
use super::ids::StepId;
use super::output::OutputDecl;

/// A single ADO step.
#[derive(Debug, Clone)]
pub enum Step {
    Bash(BashStep),
    Task(TaskStep),
    Checkout(CheckoutStep),
    Download(DownloadStep),
    Publish(PublishStep),
    /// Escape hatch for **user-authored** YAML that the IR does not
    /// model: arbitrary `setup_steps:` / `teardown_steps:` /
    /// `prepare_steps:` / engine `install_steps` content lifted
    /// verbatim from the agent's front matter or from
    /// [`crate::engine::Engine::install_steps`]. Producers live in
    /// [`crate::compile::agentic_pipeline`] (search there for
    /// `Step::RawYaml`); compiler-generated steps must use the typed
    /// variants instead — see the header comment of
    /// [`crate::compile::agentic_pipeline`] for the "no `Step::RawYaml`
    /// from generated code" rule.
    ///
    /// The string is expected to be a complete YAML mapping (e.g.
    /// `"- bash: |\n    echo hi\n  displayName: …"`); the lowering
    /// pass parses it back into a `serde_yaml::Value` and re-emits it
    /// so the canonical normalisation applies. If parsing fails the
    /// IR returns an error rather than embedding malformed YAML.
    RawYaml(String),
}

impl Step {
    /// Return this step's id, if it carries one.
    ///
    /// Steps that no other step references (the common case) do not
    /// need an id. Steps that *are* referenced via
    /// [`super::output::OutputRef`] **must** have `id: Some(_)`; the
    /// validate pass enforces this.
    pub fn id(&self) -> Option<&StepId> {
        match self {
            Step::Bash(s) => s.id.as_ref(),
            Step::Task(s) => s.id.as_ref(),
            Step::Checkout(_) => None,
            Step::Download(_) => None,
            Step::Publish(_) => None,
            // `RawYaml` is opaque user-authored YAML; the IR cannot
            // introspect any embedded `name:` key. Producers that
            // need cross-step refs must use a typed variant.
            Step::RawYaml(_) => None,
        }
    }
}

/// A bash step (`- bash: |\n    <body>`).
#[derive(Debug, Clone)]
pub struct BashStep {
    /// ADO step `name:` — required iff any other step references
    /// this step's outputs via [`super::output::OutputRef`].
    pub id: Option<StepId>,
    /// ADO step `displayName:`.
    pub display_name: String,
    /// Raw bash body — no leading `- bash: |`, no per-line indent.
    /// The YAML emit pass handles literal-block wrapping.
    pub script: String,
    /// Environment-variable bindings.
    pub env: IndexMap<String, EnvValue>,
    /// Outputs declared by this step. See [`OutputDecl`] for the
    /// `isOutput=true` contract: the graph pass marks each decl with
    /// at least one cross-step reader via `auto_is_output`, but the
    /// producer's bash body is responsible for emitting the
    /// `##vso[task.setvariable …;isOutput=true]` directive itself.
    pub outputs: Vec<OutputDecl>,
    /// ADO `condition:`. `None` means "no explicit condition";
    /// ADO defaults to `succeeded()`.
    pub condition: Option<Condition>,
    /// `timeoutInMinutes:` mapped from a `Duration` for type safety.
    /// The emit pass rounds up to whole minutes.
    pub timeout: Option<Duration>,
    /// `continueOnError:` — defaults to `false`.
    pub continue_on_error: bool,
    /// `workingDirectory:` — defaults to none.
    pub working_directory: Option<String>,
}

impl BashStep {
    /// Construct a minimal bash step. Use builder-style setters on
    /// the returned value to configure id, env, outputs, etc.
    pub fn new(display_name: impl Into<String>, script: impl Into<String>) -> Self {
        Self {
            id: None,
            display_name: display_name.into(),
            script: script.into(),
            env: IndexMap::new(),
            outputs: Vec::new(),
            condition: None,
            timeout: None,
            continue_on_error: false,
            working_directory: None,
        }
    }

    /// Set the step id.
    pub fn with_id(mut self, id: StepId) -> Self {
        self.id = Some(id);
        self
    }

    /// Set the step condition.
    pub fn with_condition(mut self, c: Condition) -> Self {
        self.condition = Some(c);
        self
    }

    /// Set `continueOnError` (best-effort steps that must never fail the build).
    pub fn with_continue_on_error(mut self, yes: bool) -> Self {
        self.continue_on_error = yes;
        self
    }

    /// Add (or replace) an env-var binding.
    pub fn with_env(mut self, key: impl Into<String>, value: EnvValue) -> Self {
        self.env.insert(key.into(), value);
        self
    }

    /// Declare an output.
    pub fn with_output(mut self, decl: OutputDecl) -> Self {
        self.outputs.push(decl);
        self
    }
}

/// A `task:` step (e.g. `UseNode@1`, `UsePythonVersion@0`).
#[derive(Debug, Clone)]
pub struct TaskStep {
    pub id: Option<StepId>,
    pub display_name: String,
    /// The task identifier, e.g. `"UseNode@1"` or `"UseDotNet@2"`.
    pub task: String,
    /// `inputs:` block — emitted in insertion order.
    pub inputs: IndexMap<String, String>,
    pub env: IndexMap<String, EnvValue>,
    pub condition: Option<Condition>,
    pub timeout: Option<Duration>,
    pub continue_on_error: bool,
}

impl TaskStep {
    pub fn new(task: impl Into<String>, display_name: impl Into<String>) -> Self {
        Self {
            id: None,
            display_name: display_name.into(),
            task: task.into(),
            inputs: IndexMap::new(),
            env: IndexMap::new(),
            condition: None,
            timeout: None,
            continue_on_error: false,
        }
    }

    pub fn with_input(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.inputs.insert(key.into(), value.into());
        self
    }
}

/// A `- checkout: …` step.
#[derive(Debug, Clone)]
pub struct CheckoutStep {
    /// `self`, or a named repository resource.
    pub repository: CheckoutRepo,
    pub clean: Option<bool>,
    pub submodules: Option<SubmodulesOpt>,
    pub fetch_depth: Option<u32>,
    pub fetch_tags: Option<bool>,
    pub persist_credentials: Option<bool>,
}

/// Target of a [`CheckoutStep`].
#[derive(Debug, Clone)]
pub enum CheckoutRepo {
    /// `checkout: self` — the trigger repository.
    Self_,
    /// `checkout: none` — explicitly disable repository checkout.
    None,
    /// `checkout: <resource_name>` — a named repository resource.
    Named(String),
}

/// `submodules:` option for a [`CheckoutStep`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmodulesOpt {
    True,
    Recursive,
    False,
}

/// A `- download: …` step (pipeline-artifact download).
#[derive(Debug, Clone)]
pub struct DownloadStep {
    /// `current` for same-pipeline artifacts; a pipeline-resource
    /// name otherwise.
    pub source: String,
    /// `artifact: <name>`.
    pub artifact: String,
    pub condition: Option<Condition>,
}

/// A `- publish: <path>` step.
#[derive(Debug, Clone)]
pub struct PublishStep {
    pub path: String,
    pub artifact: String,
    pub condition: Option<Condition>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_step_builder_round_trip() {
        let s = BashStep::new("ado-aw", "echo hi")
            .with_id(StepId::new("marker").unwrap())
            .with_env("FOO", EnvValue::literal("bar"))
            .with_output(OutputDecl::new("AW_OUT"));
        assert_eq!(s.display_name, "ado-aw");
        assert_eq!(s.script, "echo hi");
        assert_eq!(s.id.as_ref().map(|i| i.as_str()), Some("marker"));
        // Verify the actual key and value were stored, not just the count.
        assert_eq!(
            s.env.get("FOO"),
            Some(&EnvValue::literal("bar")),
            "env should map FOO -> literal(\"bar\")"
        );
        assert_eq!(s.outputs.len(), 1, "should have exactly one output");
        assert_eq!(s.outputs[0].name, "AW_OUT", "output name should be AW_OUT");
    }

    #[test]
    fn step_id_returns_none_for_anchorless_kinds() {
        let chk = Step::Checkout(CheckoutStep {
            repository: CheckoutRepo::Self_,
            clean: None,
            submodules: None,
            fetch_depth: None,
            fetch_tags: None,
            persist_credentials: None,
        });
        assert!(chk.id().is_none());

        let dl = Step::Download(DownloadStep {
            source: "current".into(),
            artifact: "agent_outputs".into(),
            condition: None,
        });
        assert!(dl.id().is_none());
    }

    #[test]
    fn step_id_returns_inner_for_bash_with_id() {
        let bs = BashStep::new("d", "true").with_id(StepId::new("synthPr").unwrap());
        let s = Step::Bash(bs);
        assert_eq!(s.id().map(|i| i.as_str()), Some("synthPr"));
    }

    #[test]
    fn task_step_builder_adds_inputs() {
        let t = TaskStep::new("UseNode@1", "Install Node.js 20.x").with_input("version", "20.x");
        assert_eq!(t.task, "UseNode@1");
        assert_eq!(t.inputs.get("version").map(|s| s.as_str()), Some("20.x"));
    }

    #[test]
    fn raw_yaml_step_carries_no_id() {
        let s = Step::RawYaml("- bash: echo hi\n  displayName: hi".into());
        assert!(s.id().is_none());
    }
}
