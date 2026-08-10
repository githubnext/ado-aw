//! Shellcheck every registered script, in isolation.
//!
//! # Why this sits next to the registry rather than in `tests/`
//!
//! `tests/bash_lint_tests.rs` lints the shell that *reached* the emitted YAML,
//! which is the right check for "is what we ship correct" but makes coverage a
//! function of fixture reachability. A generator no fixture exercises is
//! linted by nothing — which is how several hundred lines of `ado-proxy` and
//! `az` wrapper shell went unlinted.
//!
//! This harness reads [`super::registry::all_scripts`] directly, so it sees
//! every script whether or not any pipeline emits it. The two are
//! complementary and both are kept: this one proves the shell is *correct*,
//! the integration test proves it is *emitted*.
//!
//! Skips when `shellcheck` is absent unless `ENFORCE_BASH_LINT` is set, which
//! CI does — matching the integration test's behaviour exactly.

use std::io::Write;
use std::process::{Command, Stdio};

use serde::Deserialize;

use super::registry::{ShellScriptDef, all_scripts};
use super::FRAGMENT_MARKER;
use super::bindings::is_shell_var_name;

/// One shellcheck JSON finding.
#[derive(Debug, Deserialize)]
struct Finding {
    line: u64,
    level: String,
    code: u64,
    message: String,
}

fn shellcheck_available() -> bool {
    Command::new("shellcheck")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Run shellcheck over one script's lint source.
fn check(def: &ShellScriptDef) -> Vec<Finding> {
    let mut child = Command::new("shellcheck")
        .arg(format!("--shell={}", def.interpreter.shellcheck_dialect()))
        .arg("--format=json")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn shellcheck");
    child
        .stdin
        .take()
        .expect("shellcheck stdin")
        .write_all(def.lint_source().as_bytes())
        .expect("write script to shellcheck");
    let output = child.wait_with_output().expect("await shellcheck");
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "shellcheck produced unparseable output for {}: {e}\nstdout: {}\nstderr: {}",
            def.name,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        )
    })
}

#[test]
fn every_registered_script_passes_shellcheck() {
    if !shellcheck_available() {
        assert!(
            std::env::var_os("ENFORCE_BASH_LINT").is_none(),
            "ENFORCE_BASH_LINT is set but shellcheck is not on PATH"
        );
        eprintln!("note: shellcheck not found; skipping. Set ENFORCE_BASH_LINT to enforce.");
        return;
    }

    let mut report = String::new();
    for def in all_scripts() {
        let findings: Vec<Finding> = check(def)
            .into_iter()
            .filter(|f| f.level == "error" || f.level == "warning")
            .collect();
        if findings.is_empty() {
            continue;
        }
        // Line numbers are relative to the lint source, whose stub prelude is
        // synthesised — the header below names the producing Rust source so a
        // reader can map a finding back to the raw-string body.
        report.push_str(&format!("\n{} ({}:{})\n", def.name, def.file, def.line));
        for finding in findings {
            report.push_str(&format!(
                "  line {:>3}  SC{}  {}  {}\n",
                finding.line, finding.code, finding.level, finding.message
            ));
        }
    }

    assert!(
        report.is_empty(),
        "shellcheck flagged registered scripts. Fix the raw-string body, or \
         add a per-line `# shellcheck disable=SCxxxx` comment above the \
         offending line with a justification.\n{report}"
    );
}

#[test]
fn every_declared_variable_name_is_a_valid_shell_name() {
    // A binding is emitted as `NAME=value` in the prelude. A lowercase or
    // hyphenated name would either produce invalid shell or silently shadow a
    // convention, and the failure would surface at pipeline runtime rather
    // than here.
    let mut problems = String::new();
    for def in all_scripts() {
        for name in def.bindings.iter().chain(def.externals.iter()) {
            if !is_shell_var_name(name) {
                problems.push_str(&format!(
                    "  {} declares `{name}`, which is not a SCREAMING_SNAKE shell name ({}:{})\n",
                    def.name, def.file, def.line
                ));
            }
        }
    }
    assert!(problems.is_empty(), "invalid shell variable names:\n{problems}");
}

#[test]
fn every_phase_is_also_a_declared_fragment() {
    // A `phases:` entry that is not in `fragments:` would never be spliced —
    // the composed lint would silently fall back to linting an outline.
    let mut problems = String::new();
    for def in all_scripts() {
        for (name, _) in def.phases {
            if !def.fragments.contains(name) {
                problems.push_str(&format!(
                    "  {} declares phase `{name}` without declaring it as a fragment ({}:{})\n",
                    def.name, def.file, def.line
                ));
            }
        }
    }
    assert!(problems.is_empty(), "phase declaration drift:\n{problems}");
}

#[test]
fn a_composed_script_is_linted_with_its_phases_spliced() {
    // Guards the mechanism the SC2034-on-every-binding failure exposed: an
    // outline made only of markers must be linted as the script that runs,
    // not as the outline.
    let composed: Vec<&'static ShellScriptDef> = all_scripts()
        .into_iter()
        .filter(|def| !def.phases.is_empty())
        .collect();
    assert!(
        !composed.is_empty(),
        "no script declares phases; if composition was removed, remove this test too"
    );
    for def in composed {
        let source = def.lint_source();
        for (name, phase) in def.phases {
            assert!(
                !source.contains(&format!("{FRAGMENT_MARKER}{name}")),
                "{}: phase `{name}` was left as a marker instead of being spliced",
                def.name
            );
            let first = phase
                .body
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty() && !l.starts_with('#'))
                .unwrap_or_else(|| panic!("{} has no runnable line", phase.name));
            assert!(
                source.contains(first),
                "{}: phase `{name}` body is missing from the composed lint source",
                def.name
            );
        }
    }
}

#[test]
fn every_declared_fragment_has_a_marker_and_vice_versa() {
    // `splice_fragments` enforces this at render time, but only for scripts a
    // test or a compile actually renders. Checking the whole registry
    // statically means a fragment that is declared and never marked — shell
    // that was meant to run and silently would not — fails here instead.
    let mut problems = String::new();
    for def in all_scripts() {
        let marked: Vec<&str> = def
            .body
            .lines()
            .filter_map(super::fragment_marker)
            .collect();
        for name in def.fragments {
            if !marked.contains(name) {
                problems.push_str(&format!(
                    "  {} declares fragment `{name}` with no marker in the body ({}:{})\n",
                    def.name, def.file, def.line
                ));
            }
        }
        for name in &marked {
            if !def.fragments.contains(name) {
                problems.push_str(&format!(
                    "  {} marks fragment `{name}` without declaring it ({}:{})\n",
                    def.name, def.file, def.line
                ));
            }
        }
    }
    assert!(problems.is_empty(), "fragment declaration drift:\n{problems}");
}

#[test]
fn every_registered_script_declares_the_variables_it_reads() {
    // A body that reads `$FOO` without declaring FOO as a binding or an
    // external is either a typo or a variable arriving through an
    // undocumented channel. Both are worth failing on, and shellcheck's
    // SC2154 only catches it when the variable is never assigned *anywhere*
    // in the body — this catches the declaration gap directly.
    let mut undeclared = String::new();
    for def in all_scripts() {
        for name in referenced_vars(def.body) {
            let declared = def.bindings.contains(&name.as_str())
                || def.externals.contains(&name.as_str())
                || assigned_in_body(def.body, &name);
            if !declared {
                undeclared.push_str(&format!(
                    "  {} reads ${name} without declaring it ({}:{})\n",
                    def.name, def.file, def.line
                ));
            }
        }
    }
    assert!(
        undeclared.is_empty(),
        "shell scripts must declare every variable they read as a `bindings:` \
         entry (compiler-supplied) or an `externals:` entry (env / fragment / \
         setvariable):\n{undeclared}"
    );
}

/// Variable names a body references as `$NAME` or `${NAME…}`.
///
/// Deliberately only SCREAMING_SNAKE names: lowercase locals are the body's
/// own business, and shell specials (`$1`, `$@`, `$?`) are not names.
///
/// Single-quoted spans are skipped. Nothing expands inside `'…'`, so a `$NF`
/// in an embedded awk program or a `$1` in a sed replacement is not a shell
/// variable reference. Without this the checker would demand a declaration for
/// another language's variables and push authors into renaming them, which
/// distorts the script to satisfy the tool.
fn referenced_vars(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes: Vec<char> = body.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == '\'' {
            i += 1;
            while i < bytes.len() && bytes[i] != '\'' {
                i += 1;
            }
            i += 1;
            continue;
        }
        if bytes[i] != '$' {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        if j < bytes.len() && bytes[j] == '{' {
            j += 1;
        }
        let start = j;
        while j < bytes.len()
            && (bytes[j].is_ascii_uppercase()
                || bytes[j] == '_'
                || (j > start && bytes[j].is_ascii_digit()))
        {
            j += 1;
        }
        if j > start {
            let name: String = bytes[start..j].iter().collect();
            if !out.contains(&name) {
                out.push(name);
            }
        }
        i = j.max(i + 1);
    }
    out
}

/// Whether the body assigns `name` itself (`NAME=`, `for NAME in`, `read NAME`).
fn assigned_in_body(body: &str, name: &str) -> bool {
    body.lines().any(|line| {
        let line = line.trim();
        line.starts_with(&format!("{name}="))
            || line.starts_with(&format!("export {name}="))
            || line.starts_with(&format!("for {name} in"))
            || line.starts_with(&format!("read {name}"))
            || line.starts_with(&format!("read -r {name}"))
            || line.contains(&format!("; {name}="))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn referenced_vars_finds_both_spellings_and_ignores_specials() {
        let vars = referenced_vars(r#"echo "$FOO ${BAR}x $1 $@ $lower $BAZ2""#);
        assert_eq!(vars, vec!["FOO", "BAR", "BAZ2"]);
    }

    #[test]
    fn referenced_vars_skips_single_quoted_spans() {
        // `$NF` belongs to awk, not to the shell — nothing expands inside
        // '…'. Treating it as a shell variable would force the author to
        // rename another language's variable to satisfy this checker.
        let vars = referenced_vars(
            r#"awk -F/ '{ if (NF>1) print $NF }' <<< "$COLLECTION""#,
        );
        assert_eq!(vars, vec!["COLLECTION"]);
    }

    #[test]
    fn referenced_vars_resumes_after_a_closing_quote() {
        let vars = referenced_vars(r#"sed 's/$X/y/' "$REAL""#);
        assert_eq!(vars, vec!["REAL"]);
    }

    #[test]
    fn assigned_in_body_recognises_the_common_forms() {
        assert!(assigned_in_body("PROXY_DIR=$(mktemp -d)", "PROXY_DIR"));
        assert!(assigned_in_body("export PROXY_DIR=/tmp", "PROXY_DIR"));
        assert!(assigned_in_body("for PROXY_HOST in $HOSTS; do", "PROXY_HOST"));
        assert!(assigned_in_body("set -eu; UMASK=1", "UMASK"));
        assert!(!assigned_in_body("echo \"$PROXY_DIR\"", "PROXY_DIR"));
    }
}
