//! Integration test that lints the bash bodies of compiled pipeline YAML
//! using `shellcheck`.
//!
//! ## Why this test exists
//!
//! Pipeline templates contain dozens of multi-line `bash:` steps. ADO bash
//! steps fail only on the *last* command's exit code by default, which makes
//! it easy for an earlier command to fail silently and the step to still
//! report green. Rather than spread `set -eo pipefail` boilerplate across
//! every step, we lint each bash body with shellcheck. Real silent-failure
//! patterns surface here:
//!
//! * **SC2164** — `cd $X` without `|| exit` (the canonical silent-failure)
//! * **SC2155** — `local var=$(cmd)` masking the inner exit code
//! * **SC2086 / SC2046** — unquoted variables / command substitutions
//! * **SC2154** — variables referenced but never assigned
//! * **SC2088** — tilde inside double quotes (does not expand)
//!
//! ## How it works
//!
//! 1. Compiles a representative set of fixtures with `ado-aw compile`.
//! 2. For each generated `*.lock.yml`, walks the YAML and collects every
//!    `bash:` body that is the value of a step entry (i.e., a mapping that
//!    is itself an element of a sequence). This avoids false positives from
//!    arbitrary `bash` keys nested inside `env:` blocks or comments.
//! 3. Pipes each body to `shellcheck --shell=bash --format=json -`.
//! 4. Aggregates findings; the test fails with a structured report listing
//!    every finding by fixture / step / line / code / message.
//!
//! ## Skip vs. enforce
//!
//! By default, if `shellcheck` is not installed locally the test prints a
//! notice and returns early. CI runners are expected to set the
//! `ENFORCE_BASH_LINT` environment variable so a missing shellcheck becomes
//! a hard failure rather than a silent skip. To install shellcheck locally:
//!
//! * macOS: `brew install shellcheck`
//! * Debian / Ubuntu: `apt-get install -y shellcheck`

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_yaml::Value;
use tempfile::TempDir;

/// Shellcheck rule codes that are intentionally suppressed for ADO bash steps.
///
/// Each entry has a justification — do not extend this list without one.
/// The list is deliberately short: project-specific suppressions belong as
/// per-line `# shellcheck disable=SCxxxx` comments inside the bash body, not
/// as a global override.
///
/// * **SC1090, SC1091** — `source` paths that include ADO macros
///   (e.g. `$(Pipeline.Workspace)`) are dynamic and cannot be resolved by
///   shellcheck.
const SHELLCHECK_EXCLUDE: &str = "SC1090,SC1091";

/// Fixtures exercised by the lint. Chosen to collectively cover every bash-step
/// generator in the codebase: standalone + 1ES templates, every runtime that
/// emits bash steps (Lean, Node with feed-url, .NET with feed-url), and every
/// first-class tool that emits bash (cache-memory). Add a fixture here only
/// when a new generator is introduced that none of the existing fixtures
/// exercises.
///
/// Note: `runtime-coverage-agent.md` and `runtime-coverage-1es-agent.md` are
/// the same agent compiled to different targets so we exercise code-generated
/// runtime/tool bash on both `standalone` and `1es`. Today their bash bodies
/// are byte-identical, but a future target-specific divergence in a generator
/// would only be caught with both fixtures in the harness.
const FIXTURES: &[&str] = &[
    "minimal-agent.md",
    "complete-agent.md",
    "1es-test-agent.md",
    "azure-devops-mcp-agent.md",
    "pipeline-trigger-agent.md",
    "pipeline-filter-agent.md",
    "runtime-coverage-agent.md",
    "runtime-coverage-1es-agent.md",
];

/// Step display names that the lint expects to find at least once across all
/// fixtures. If any of these is missing it means the corresponding generator
/// is not being exercised — almost always because a fixture was deleted or
/// the generator's output changed without updating the coverage list.
const REQUIRED_STEP_DISPLAY_NAMES: &[&str] = &[
    // Static templates (standalone + 1ES)
    "Prepare MCPG config",
    "Prepare tooling",
    "Prepare agent prompt",
    "Run copilot (AWF network isolated)",
    "Run threat analysis (AWF network isolated)",
    "Evaluate threat analysis",
    "Execute safe outputs (Stage 3)",
    // Rust generators
    "Install Lean 4 (elan)",                  // src/runtimes/lean/mod.rs
    "Append Lean 4 prompt",                   // src/runtimes/lean/extension.rs
    "Ensure .npmrc exists",                   // src/runtimes/node/mod.rs
    "Ensure nuget.config exists",             // src/runtimes/dotnet/mod.rs
    "Restore previous agent memory",          // src/tools/cache_memory/extension.rs
    "Initialize empty agent memory (clearMemory=true)",
    "Generate GITHUB_PATH file",              // src/compile/common.rs (AWF path step)
];

fn ado_aw_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"))
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Probe for the shellcheck binary. Returns `None` if it is not on PATH or
/// fails to report a version.
fn shellcheck_version() -> Option<String> {
    let output = Command::new("shellcheck").arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .find(|l| l.starts_with("version:"))
        .map(|l| l.trim().to_string())
}

/// A fresh `TempDir` plus a `.git/` marker so `ado-aw compile` can resolve
/// repo-relative paths. RAII cleans up on drop, even on panic.
fn fresh_workspace() -> TempDir {
    let dir = tempfile::Builder::new()
        .prefix("ado-aw-bash-lint-")
        .tempdir()
        .expect("create temp dir");
    std::fs::create_dir(dir.path().join(".git")).expect("create .git dir");
    dir
}

/// Compile a fixture by copying it into `workspace` and invoking
/// `ado-aw compile`. Returns the path to the generated `.lock.yml` and the
/// target (`"standalone"` or `"1es"`) reported in compiler stdout.
fn compile_fixture(workspace: &Path, fixture: &str) -> (PathBuf, String) {
    let src = fixtures_dir().join(fixture);
    let dest = workspace.join(fixture);
    std::fs::copy(&src, &dest)
        .unwrap_or_else(|e| panic!("copy fixture {fixture}: {e}"));

    let output = Command::new(ado_aw_binary())
        .args(["compile", dest.to_str().unwrap()])
        .current_dir(workspace)
        .output()
        .unwrap_or_else(|e| panic!("spawn ado-aw compile: {e}"));

    assert!(
        output.status.success(),
        "ado-aw compile failed for {fixture}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // `ado-aw compile` prints `Generated <target> pipeline: <file>` to stdout;
    // parse the target so the test can assert coverage of every known target.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let target = if stdout.contains("Generated 1ES pipeline:") {
        "1es"
    } else if stdout.contains("Generated standalone pipeline:") {
        "standalone"
    } else {
        panic!(
            "could not determine compile target for {fixture} from stdout:\n{stdout}"
        )
    };

    let lock = dest.with_extension("lock.yml");
    assert!(lock.exists(), "expected lock file {}", lock.display());
    (lock, target.to_string())
}

/// A single bash body extracted from a compiled pipeline.
struct BashBody {
    display_name: String,
    body: String,
}

/// Walk a parsed YAML document and collect every step that has a literal
/// block-scalar `bash:` body. Only mappings reached via a sequence element
/// are considered candidate steps — this avoids treating an arbitrary
/// `bash:` key inside, e.g., an `env:` block as a step.
fn extract_bash_bodies(yml_path: &Path) -> Vec<BashBody> {
    let content = std::fs::read_to_string(yml_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", yml_path.display()));
    let doc: Value = serde_yaml::from_str(&content)
        .unwrap_or_else(|e| panic!("parse YAML {}: {e}", yml_path.display()));

    let mut out = Vec::new();
    collect(&doc, /* in_sequence_element = */ false, &mut out);
    out
}

fn collect(node: &Value, in_sequence_element: bool, out: &mut Vec<BashBody>) {
    match node {
        Value::Mapping(map) => {
            // Only treat this mapping as a step candidate if we reached it
            // by descending into a sequence (i.e., it's `[bash: |, …]`).
            if in_sequence_element
                && let Some(Value::String(body)) = map.get(Value::String("bash".into()))
            {
                let display_name = map
                    .get(Value::String("displayName".into()))
                    .and_then(Value::as_str)
                    .unwrap_or("<unnamed>")
                    .to_string();
                out.push(BashBody {
                    display_name,
                    body: body.clone(),
                });
            }
            for (_, v) in map {
                collect(v, /* in_sequence_element = */ false, out);
            }
        }
        Value::Sequence(seq) => {
            for v in seq {
                collect(v, /* in_sequence_element = */ true, out);
            }
        }
        _ => {}
    }
}

/// Run shellcheck on a bash body. Returns the parsed JSON findings.
fn run_shellcheck(body: &str) -> serde_json::Value {
    let mut child = Command::new("shellcheck")
        .args([
            "--shell=bash",
            "--format=json",
            &format!("--exclude={SHELLCHECK_EXCLUDE}"),
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn shellcheck");

    child
        .stdin
        .as_mut()
        .expect("shellcheck stdin")
        .write_all(body.as_bytes())
        .expect("write to shellcheck stdin");
    drop(child.stdin.take());

    let output = child.wait_with_output().expect("wait for shellcheck");

    // shellcheck exits 0 when clean and 1 when findings exist; both produce
    // valid JSON on stdout. Higher exit codes (e.g. parse error) are real
    // failures and should surface in the test output.
    let exit = output.status.code().unwrap_or(-1);
    if exit > 1 {
        panic!(
            "shellcheck failed (exit {exit}):\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return serde_json::Value::Array(Vec::new());
    }

    serde_json::from_str(trimmed)
        .unwrap_or_else(|e| panic!("parse shellcheck JSON: {e}\nraw:\n{stdout}"))
}

/// Format a single shellcheck finding for the test failure report.
fn format_finding(fixture: &str, display_name: &str, finding: &serde_json::Value) -> String {
    let code = finding["code"].as_i64().unwrap_or(0);
    let level = finding["level"].as_str().unwrap_or("?");
    let line = finding["line"].as_i64().unwrap_or(0);
    let message = finding["message"].as_str().unwrap_or("?");
    format!("  [{level}] SC{code} {fixture} :: {display_name:?} (body L{line}): {message}")
}

#[test]
fn compiled_bash_bodies_pass_shellcheck() {
    let enforce = std::env::var_os("ENFORCE_BASH_LINT").is_some();

    let Some(version) = shellcheck_version() else {
        if enforce {
            panic!(
                "ENFORCE_BASH_LINT is set but `shellcheck` is not on PATH. \
                 Install it in CI (e.g. `apt-get install -y shellcheck`) \
                 or unset ENFORCE_BASH_LINT for local development."
            );
        }
        eprintln!(
            "skipping bash lint test: `shellcheck` not found on PATH. \
             Install via your OS package manager (e.g. `brew install shellcheck`, \
             `apt-get install -y shellcheck`). \
             Set ENFORCE_BASH_LINT=1 to make this a hard failure (CI does)."
        );
        return;
    };
    eprintln!("using {version}");

    let workspace = fresh_workspace();
    let mut report: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut all_display_names: Vec<String> = Vec::new();
    let mut targets_seen: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::new();

    for fixture in FIXTURES {
        let (lock, target) = compile_fixture(workspace.path(), fixture);
        targets_seen.insert(target);
        for body in extract_bash_bodies(&lock) {
            all_display_names.push(body.display_name.clone());
            let findings = run_shellcheck(&body.body);
            if let Some(arr) = findings.as_array() {
                for finding in arr {
                    report
                        .entry((*fixture).to_string())
                        .or_default()
                        .push(format_finding(fixture, &body.display_name, finding));
                }
            }
        }
    }

    // Target coverage — assert that every known compile target is exercised by
    // at least one fixture, so we shellcheck the bash output of every template
    // (`src/data/base.yml` and `src/data/1es-base.yml`) and every code-generated
    // step on both targets.
    const REQUIRED_TARGETS: &[&str] = &["standalone", "1es"];
    let missing_targets: Vec<&str> = REQUIRED_TARGETS
        .iter()
        .copied()
        .filter(|t| !targets_seen.contains(*t))
        .collect();
    assert!(
        missing_targets.is_empty(),
        "no fixture compiles to the following target(s): {:?}\n\
         Each compile target has its own template under src/data/ whose bash \
         bodies need shellchecking. Add a fixture with `target: <missing>` to \
         tests/fixtures/ and list it in FIXTURES.",
        missing_targets
    );

    // Coverage check — every required generator must appear in the harvested
    // step list, otherwise a fixture has stopped exercising its generator.
    let missing: Vec<&str> = REQUIRED_STEP_DISPLAY_NAMES
        .iter()
        .copied()
        .filter(|name| !all_display_names.iter().any(|d| d == name))
        .collect();
    assert!(
        missing.is_empty(),
        "the following step display names were not produced by any fixture, \
         meaning their generator is not being linted:\n  {}\n\
         Either add a fixture exercising the generator, or update \
         REQUIRED_STEP_DISPLAY_NAMES if the generator was removed.",
        missing.join("\n  ")
    );

    if !report.is_empty() {
        let mut msg = String::from(
            "shellcheck flagged silent-failure patterns in compiled bash bodies. \
             Each finding represents a real or stylistic concern; fix the \
             offending bash, or if intentional add `# shellcheck disable=SCxxxx` \
             inline in the bash body (the directive is a comment and does not \
             affect runtime behaviour).\n",
        );
        for (fixture, lines) in &report {
            msg.push_str(&format!("\n--- {fixture} ---\n"));
            for line in lines {
                msg.push_str(line);
                msg.push('\n');
            }
        }
        panic!("{msg}");
    }
}

/// Sanity check: every listed fixture exists. Catches typos in `FIXTURES`
/// without paying the cost of compiling.
#[test]
fn every_listed_fixture_exists() {
    for fixture in FIXTURES {
        let path = fixtures_dir().join(fixture);
        assert!(
            path.exists(),
            "fixture listed in FIXTURES but not on disk: {}",
            path.display()
        );
    }
}
