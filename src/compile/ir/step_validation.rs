//! Structural validator for **untrusted, agent-proposed step blocks**.
//!
//! This module's job is narrow and security-driven: given a chunk of
//! YAML that an agent (or anyone untrusted) has proposed for the
//! front-matter `steps:` / `post-steps:` / `setup:` / `teardown:`
//! sections, decide whether it is:
//!
//! 1. **Structurally** a list of well-formed ADO step entries that
//!    match one of the kinds the IR models in [`super::step::Step`]:
//!    `bash`, `task`, `checkout`, `download`, `publish`.
//! 2. **Restricted** to the allow-list the caller passed in. The
//!    `propose-step-optimization` safe-output uses
//!    [`StepKindAllow::Curated`], which limits `task:` steps to the
//!    set of task identifiers we expose a typed factory for in
//!    [`super::tasks`] — see [`super::tasks::CURATED_TASK_IDS`].
//! 3. **Free of obvious shape-injection footguns** — unknown
//!    step-level keys, non-string `env:` values, oversized bash
//!    bodies, etc. are rejected up front rather than smuggled past
//!    Stage 2 by virtue of being valid YAML.
//!
//! This validator deliberately does **not** lower the YAML into a
//! `Vec<Step>` first. The IR has no public `Value -> Step` parser
//! (the IR is build-only — typed Step + lower + emit), and the
//! production code path that consumes `steps:` today treats them as
//! opaque `serde_yaml::Value` passed straight to ADO. Mirroring that
//! contract here keeps the surface aligned: the validator says "ADO
//! will accept this AND it stays inside our allow-list", which is
//! exactly what Stage 3 needs before applying a proposal to the
//! source `.md`.
//!
//! For Flow A (`validate_steps` MCP tool), authors call with
//! [`StepKindAllow::Full`]; for Flow B (Stage 3 of
//! `propose-step-optimization`), the executor calls with
//! [`StepKindAllow::Curated`].

use serde::Serialize;
use serde_yaml::Value;

use super::tasks::is_curated_task;

/// How permissive the validator should be about which step kinds and
/// which `task:` identifiers are allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKindAllow {
    /// Accept every step kind the IR models (`bash`, `task`,
    /// `checkout`, `download`, `publish`) and accept **any** valid
    /// `task:` identifier (`Name@Version`). Used by Flow A
    /// (`validate_steps` MCP tool) where the human author is in the
    /// loop.
    Full,
    /// Accept only `bash:` steps and `task:` steps whose identifier
    /// appears in [`super::tasks::CURATED_TASK_IDS`]. Used by Flow B
    /// (`propose-step-optimization` Stage 3 executor) where the
    /// proposal comes from an untrusted agent.
    Curated,
}

/// The kind of an individual step, inferred from its unique kind key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum StepKind {
    /// A `bash:` step.
    Bash,
    /// A `task:` step. Carries the task identifier (e.g. `"CopyFiles@2"`).
    Task(String),
    /// A `checkout:` step.
    Checkout,
    /// A `download:` step.
    Download,
    /// A `publish:` step.
    Publish,
}

/// Result of a successful validation.
#[derive(Debug, Clone)]
pub struct ValidatedStepBlock {
    /// The original (normalised) step entries. Today this is a
    /// passthrough of the input; reserved for future canonicalisation
    /// (e.g. trimming trailing whitespace from bash bodies).
    pub steps: Vec<Value>,
    /// One [`StepKind`] per entry, in the same order as `steps`.
    pub kinds: Vec<StepKind>,
}

/// A single, addressable validation failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StepValidationError {
    /// Zero-based index of the offending step entry in the input
    /// sequence. `usize::MAX` is used for whole-block errors (e.g.
    /// "input is not a sequence").
    pub step_index: usize,
    /// Dotted path inside the step where the failure was detected
    /// (e.g. `"steps[1].inputs.targetFolder"`).
    pub path: String,
    /// Human-readable failure message.
    pub message: String,
}

impl StepValidationError {
    fn block(message: impl Into<String>) -> Self {
        Self {
            step_index: usize::MAX,
            path: "steps".into(),
            message: message.into(),
        }
    }

    fn at(step_index: usize, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            step_index,
            path: path.into(),
            message: message.into(),
        }
    }
}

/// Cap on the raw byte length of a single `bash:` script body. The
/// `propose-step-optimization` flow is intended for small,
/// deterministic snippets (clone, install, restore); anything larger
/// is more likely a misuse than a real proposal.
const MAX_BASH_BODY_BYTES: usize = 10_000;

/// Known optional step-level keys allowed across every step kind.
const COMMON_OPTIONAL_KEYS: &[&str] = &[
    "displayName",
    "name",
    "condition",
    "continueOnError",
    "timeoutInMinutes",
    "enabled",
];

/// Keys accepted on a `bash:` step in addition to [`COMMON_OPTIONAL_KEYS`].
const BASH_EXTRA_KEYS: &[&str] = &["env", "workingDirectory", "failOnStderr"];

/// Keys accepted on a `task:` step in addition to [`COMMON_OPTIONAL_KEYS`].
const TASK_EXTRA_KEYS: &[&str] = &["inputs", "env"];

/// Keys accepted on a `checkout:` step in addition to [`COMMON_OPTIONAL_KEYS`].
const CHECKOUT_EXTRA_KEYS: &[&str] = &[
    "clean",
    "fetchDepth",
    "fetchTags",
    "lfs",
    "persistCredentials",
    "submodules",
    "path",
];

/// Keys accepted on a `download:` step in addition to [`COMMON_OPTIONAL_KEYS`].
const DOWNLOAD_EXTRA_KEYS: &[&str] = &["artifact", "patterns", "path"];

/// Keys accepted on a `publish:` step in addition to [`COMMON_OPTIONAL_KEYS`].
const PUBLISH_EXTRA_KEYS: &[&str] = &["artifact"];

/// Validate `value` as an agent-proposed step block.
///
/// Returns the validated structure on success or **all** detected
/// errors at once on failure (validation does not short-circuit on
/// the first issue — callers get the full picture so they can
/// surface comprehensive feedback to authors and agents).
pub fn validate_step_block(
    value: &Value,
    allow: StepKindAllow,
) -> Result<ValidatedStepBlock, Vec<StepValidationError>> {
    let Value::Sequence(seq) = value else {
        return Err(vec![StepValidationError::block(
            "expected a YAML sequence of step entries",
        )]);
    };

    let mut errors = Vec::new();
    let mut kinds = Vec::with_capacity(seq.len());

    for (i, step) in seq.iter().enumerate() {
        match classify_and_validate_step(i, step, allow) {
            Ok(kind) => kinds.push(kind),
            Err(mut es) => {
                // Push a placeholder so kinds.len() == seq.len() on the error path,
                // making error indexing predictable for callers that iterate both.
                kinds.push(StepKind::Bash);
                errors.append(&mut es);
            }
        }
    }

    if errors.is_empty() {
        Ok(ValidatedStepBlock {
            steps: seq.clone(),
            kinds,
        })
    } else {
        Err(errors)
    }
}

fn classify_and_validate_step(
    index: usize,
    step: &Value,
    allow: StepKindAllow,
) -> Result<StepKind, Vec<StepValidationError>> {
    let Value::Mapping(map) = step else {
        return Err(vec![StepValidationError::at(
            index,
            format!("steps[{index}]"),
            "step entry must be a YAML mapping",
        )]);
    };

    let kind_keys = ["bash", "task", "checkout", "download", "publish"];
    let present: Vec<&str> = kind_keys
        .into_iter()
        .filter(|k| map.contains_key(Value::String((*k).into())))
        .collect();

    match present.as_slice() {
        [] => Err(vec![StepValidationError::at(
            index,
            format!("steps[{index}]"),
            format!(
                "step entry must contain exactly one of: {}",
                kind_keys.join(", ")
            ),
        )]),
        [_, _, ..] => Err(vec![StepValidationError::at(
            index,
            format!("steps[{index}]"),
            format!(
                "step entry must contain exactly one kind key (got: {})",
                present.join(", ")
            ),
        )]),
        [single] => match *single {
            "bash" => validate_bash_step(index, map),
            "task" => validate_task_step(index, map, allow),
            "checkout" => validate_simple_kind(index, map, "checkout", CHECKOUT_EXTRA_KEYS)
                .map(|_| StepKind::Checkout),
            "download" => validate_simple_kind(index, map, "download", DOWNLOAD_EXTRA_KEYS)
                .map(|_| StepKind::Download),
            "publish" => validate_simple_kind(index, map, "publish", PUBLISH_EXTRA_KEYS)
                .map(|_| StepKind::Publish),
            _ => unreachable!("filtered above"),
        },
    }
}

fn validate_bash_step(
    index: usize,
    map: &serde_yaml::Mapping,
) -> Result<StepKind, Vec<StepValidationError>> {
    let mut errors = Vec::new();
    let body = map.get(Value::String("bash".into())).unwrap();
    let Value::String(script) = body else {
        return Err(vec![StepValidationError::at(
            index,
            format!("steps[{index}].bash"),
            "bash: value must be a string (the script body)",
        )]);
    };

    if script.len() > MAX_BASH_BODY_BYTES {
        errors.push(StepValidationError::at(
            index,
            format!("steps[{index}].bash"),
            format!(
                "bash body exceeds {MAX_BASH_BODY_BYTES}-byte cap ({} bytes)",
                script.len()
            ),
        ));
    }

    check_extra_keys(index, map, "bash", BASH_EXTRA_KEYS, &mut errors);
    check_env_string_values(index, map, "bash", &mut errors);

    if errors.is_empty() {
        Ok(StepKind::Bash)
    } else {
        Err(errors)
    }
}

fn validate_task_step(
    index: usize,
    map: &serde_yaml::Mapping,
    allow: StepKindAllow,
) -> Result<StepKind, Vec<StepValidationError>> {
    let mut errors = Vec::new();

    let task_value = map.get(Value::String("task".into())).unwrap();
    let Value::String(task_id) = task_value else {
        return Err(vec![StepValidationError::at(
            index,
            format!("steps[{index}].task"),
            "task: value must be a string of the form Name@Version",
        )]);
    };

    if !is_valid_task_identifier(task_id) {
        errors.push(StepValidationError::at(
            index,
            format!("steps[{index}].task"),
            format!(
                "task identifier {task_id:?} is malformed (expected Name@Version, e.g. CopyFiles@2)"
            ),
        ));
    } else if matches!(allow, StepKindAllow::Curated) && !is_curated_task(task_id) {
        errors.push(StepValidationError::at(
            index,
            format!("steps[{index}].task"),
            format!(
                "task {task_id:?} is not in the curated allow-list \
                 (Curated mode only permits tasks with a typed \
                 factory in src/compile/ir/tasks.rs)"
            ),
        ));
    }

    if let Some(inputs) = map.get(Value::String("inputs".into())) {
        match inputs {
            Value::Mapping(im) => {
                for (k, v) in im {
                    if !matches!(k, Value::String(_)) {
                        errors.push(StepValidationError::at(
                            index,
                            format!("steps[{index}].inputs"),
                            "task input keys must be strings",
                        ));
                    }
                    if !is_scalar_string_or_primitive(v) {
                        let key_label = key_label(k);
                        errors.push(StepValidationError::at(
                            index,
                            format!("steps[{index}].inputs.{key_label}"),
                            "task input values must be scalars (strings/bools/numbers)",
                        ));
                    }
                }
            }
            _ => errors.push(StepValidationError::at(
                index,
                format!("steps[{index}].inputs"),
                "inputs: must be a mapping of string keys to scalar values",
            )),
        }
    }

    check_extra_keys(index, map, "task", TASK_EXTRA_KEYS, &mut errors);
    check_env_string_values(index, map, "task", &mut errors);

    if errors.is_empty() {
        Ok(StepKind::Task(task_id.clone()))
    } else {
        Err(errors)
    }
}

fn validate_simple_kind(
    index: usize,
    map: &serde_yaml::Mapping,
    kind: &str,
    extra_keys: &[&str],
) -> Result<(), Vec<StepValidationError>> {
    let mut errors = Vec::new();
    let v = map.get(Value::String(kind.into())).unwrap();
    if !is_scalar_string_or_primitive(v) {
        errors.push(StepValidationError::at(
            index,
            format!("steps[{index}].{kind}"),
            format!("{kind}: value must be a scalar string"),
        ));
    }
    check_extra_keys(index, map, kind, extra_keys, &mut errors);

    if errors.is_empty() { Ok(()) } else { Err(errors) }
}

/// Confirm every key in the step mapping is either the kind key, a
/// known common optional key, or one of the per-kind extras.
/// Unknown keys are rejected — they're the most common
/// shape-injection vector in untrusted YAML.
fn check_extra_keys(
    index: usize,
    map: &serde_yaml::Mapping,
    kind: &str,
    extra_keys: &[&str],
    errors: &mut Vec<StepValidationError>,
) {
    for (k, _) in map {
        let Value::String(key) = k else {
            errors.push(StepValidationError::at(
                index,
                format!("steps[{index}]"),
                "step mapping keys must be strings",
            ));
            continue;
        };
        if key == kind {
            continue;
        }
        if COMMON_OPTIONAL_KEYS.contains(&key.as_str()) {
            continue;
        }
        if extra_keys.contains(&key.as_str()) {
            continue;
        }
        errors.push(StepValidationError::at(
            index,
            format!("steps[{index}].{key}"),
            format!(
                "unknown key {key:?} on {kind} step (allowed: {}, plus the common keys: {})",
                extra_keys.join(", "),
                COMMON_OPTIONAL_KEYS.join(", "),
            ),
        ));
    }
}

/// `env:` must be a mapping whose every value is a scalar string.
/// Nested objects or sequences are an injection vector because they
/// can smuggle ADO macros or template expressions; we accept only
/// inert string scalars.
fn check_env_string_values(
    index: usize,
    map: &serde_yaml::Mapping,
    kind: &str,
    errors: &mut Vec<StepValidationError>,
) {
    let Some(env) = map.get(Value::String("env".into())) else {
        return;
    };
    let Value::Mapping(env_map) = env else {
        errors.push(StepValidationError::at(
            index,
            format!("steps[{index}].env"),
            format!("{kind} env: must be a mapping of string keys to string values"),
        ));
        return;
    };
    for (k, v) in env_map {
        let key_label = key_label(k);
        if !matches!(k, Value::String(_)) {
            errors.push(StepValidationError::at(
                index,
                format!("steps[{index}].env"),
                "env keys must be strings",
            ));
        }
        if !matches!(v, Value::String(_)) {
            errors.push(StepValidationError::at(
                index,
                format!("steps[{index}].env.{key_label}"),
                "env values must be string scalars",
            ));
        }
    }
}

/// Lax validator for ADO task identifiers (`Name@Version`).
/// Matches the same shape ADO accepts: an identifier-y leading name
/// followed by `@` and one or more digits.
fn is_valid_task_identifier(s: &str) -> bool {
    let Some((name, version)) = s.split_once('@') else {
        return false;
    };
    if name.is_empty() || version.is_empty() {
        return false;
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return false;
    }
    if !name.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    version.chars().all(|c| c.is_ascii_digit())
}

fn is_scalar_string_or_primitive(v: &Value) -> bool {
    matches!(
        v,
        Value::String(_) | Value::Bool(_) | Value::Number(_) | Value::Null
    )
}

fn key_label(k: &Value) -> String {
    match k {
        Value::String(s) => s.clone(),
        other => serde_yaml::to_string(other).unwrap_or_else(|_| "<key>".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(yaml: &str) -> Value {
        serde_yaml::from_str(yaml).expect("test YAML parses")
    }

    // ── Happy paths ───────────────────────────────────────────────────

    #[test]
    fn accepts_minimal_bash_step_in_curated_mode() {
        let y = parse("- bash: echo hi\n  displayName: Greeting\n");
        let r = validate_step_block(&y, StepKindAllow::Curated).unwrap();
        assert_eq!(r.kinds, vec![StepKind::Bash]);
    }

    #[test]
    fn accepts_curated_task_step_in_curated_mode() {
        let y = parse(
            "- task: CopyFiles@2\n  displayName: Copy\n  inputs:\n    Contents: '**/*.rs'\n    TargetFolder: out\n",
        );
        let r = validate_step_block(&y, StepKindAllow::Curated).unwrap();
        assert_eq!(r.kinds, vec![StepKind::Task("CopyFiles@2".into())]);
    }

    #[test]
    fn accepts_full_mode_with_arbitrary_task() {
        let y = parse(
            "- task: UseNode@1\n  displayName: Setup Node\n  inputs:\n    version: '20.x'\n",
        );
        let r = validate_step_block(&y, StepKindAllow::Full).unwrap();
        assert_eq!(r.kinds, vec![StepKind::Task("UseNode@1".into())]);
    }

    #[test]
    fn accepts_multi_entry_mixed_step_block() {
        let y = parse(
            "- bash: echo prepare\n- task: CopyFiles@2\n  inputs:\n    Contents: '**'\n    TargetFolder: out\n- publish: out\n  artifact: drop\n",
        );
        let r = validate_step_block(&y, StepKindAllow::Curated).unwrap();
        assert_eq!(
            r.kinds,
            vec![
                StepKind::Bash,
                StepKind::Task("CopyFiles@2".into()),
                StepKind::Publish,
            ]
        );
    }

    #[test]
    fn accepts_checkout_and_download_in_full_mode() {
        let y = parse(
            "- checkout: self\n  fetchDepth: 1\n- download: current\n  artifact: drop\n",
        );
        let r = validate_step_block(&y, StepKindAllow::Full).unwrap();
        assert_eq!(r.kinds, vec![StepKind::Checkout, StepKind::Download]);
    }

    // ── Failure paths ─────────────────────────────────────────────────

    #[test]
    fn rejects_top_level_mapping() {
        let y = parse("foo: bar\n");
        let errs = validate_step_block(&y, StepKindAllow::Curated).unwrap_err();
        assert_eq!(errs.len(), 1);
        assert!(errs[0].message.contains("sequence"));
    }

    #[test]
    fn rejects_step_entry_without_kind_key() {
        let y = parse("- displayName: orphan\n");
        let errs = validate_step_block(&y, StepKindAllow::Curated).unwrap_err();
        assert_eq!(errs[0].step_index, 0);
        assert!(errs[0].message.contains("exactly one of"));
    }

    #[test]
    fn rejects_step_entry_with_multiple_kind_keys() {
        let y = parse("- bash: echo hi\n  task: CopyFiles@2\n");
        let errs = validate_step_block(&y, StepKindAllow::Curated).unwrap_err();
        assert!(errs[0].message.contains("exactly one kind key"));
    }

    #[test]
    fn curated_mode_rejects_uncurated_task() {
        let y = parse("- task: AzureCLI@2\n  displayName: oops\n");
        let errs = validate_step_block(&y, StepKindAllow::Curated).unwrap_err();
        assert!(
            errs.iter().any(|e| e.message.contains("curated allow-list")),
            "expected curated-rejection error; got: {errs:?}"
        );
    }

    #[test]
    fn full_mode_accepts_uncurated_task_but_still_validates_format() {
        let y_valid = parse("- task: AzureCLI@2\n");
        assert!(validate_step_block(&y_valid, StepKindAllow::Full).is_ok());

        let y_bad = parse("- task: not_a_valid_id\n");
        let errs = validate_step_block(&y_bad, StepKindAllow::Full).unwrap_err();
        assert!(errs[0].message.contains("malformed"));
    }

    #[test]
    fn rejects_unknown_step_level_key() {
        let y = parse("- bash: echo hi\n  evilKey: gotcha\n");
        let errs = validate_step_block(&y, StepKindAllow::Curated).unwrap_err();
        assert!(
            errs.iter().any(|e| e.message.contains("unknown key")),
            "expected unknown-key error; got: {errs:?}"
        );
    }

    #[test]
    fn rejects_nonstring_env_value() {
        let y = parse("- bash: echo hi\n  env:\n    K:\n      nested: bad\n");
        let errs = validate_step_block(&y, StepKindAllow::Curated).unwrap_err();
        assert!(errs.iter().any(|e| e.path.contains("env.K")));
    }

    #[test]
    fn rejects_bash_body_exceeding_cap() {
        let body = "x".repeat(MAX_BASH_BODY_BYTES + 1);
        let y = parse(&format!("- bash: {body}\n"));
        let errs = validate_step_block(&y, StepKindAllow::Curated).unwrap_err();
        assert!(errs[0].message.contains("exceeds"));
    }

    #[test]
    fn rejects_task_with_nonscalar_input_value() {
        let y = parse(
            "- task: CopyFiles@2\n  inputs:\n    Contents: ['**']\n    TargetFolder: out\n",
        );
        let errs = validate_step_block(&y, StepKindAllow::Curated).unwrap_err();
        assert!(
            errs.iter().any(|e| e.path.contains("inputs.Contents")),
            "expected inputs.Contents error; got: {errs:?}"
        );
    }

    #[test]
    fn collects_errors_from_multiple_steps_not_short_circuiting() {
        let y = parse(
            "- task: AzureCLI@2\n- bash: echo ok\n  unknown: yes\n- task: malformed_id\n",
        );
        let errs = validate_step_block(&y, StepKindAllow::Curated).unwrap_err();
        // One error from each bad step (indices 0, 1, 2)
        let indices: std::collections::BTreeSet<_> =
            errs.iter().map(|e| e.step_index).collect();
        assert!(indices.contains(&0));
        assert!(indices.contains(&1));
        assert!(indices.contains(&2));
    }

    // ── Task identifier parser ────────────────────────────────────────

    #[test]
    fn task_identifier_accepts_well_formed_ids() {
        for id in &[
            "CopyFiles@2",
            "UseNode@1",
            "UsePythonVersion@0",
            "Cache@2",
            "DownloadPipelineArtifact@2",
            "PublishPipelineArtifact@1",
        ] {
            assert!(
                is_valid_task_identifier(id),
                "expected {id} to be valid"
            );
        }
    }

    #[test]
    fn task_identifier_rejects_malformed() {
        for id in &[
            "CopyFiles",          // no @version
            "@2",                 // empty name
            "CopyFiles@",         // empty version
            "Copy Files@2",       // space in name
            "1CopyFiles@2",       // leading digit
            "CopyFiles@v2",       // non-digit version
            "Copy-Files@2",       // hyphen in name
        ] {
            assert!(
                !is_valid_task_identifier(id),
                "expected {id} to be rejected"
            );
        }
    }
}
