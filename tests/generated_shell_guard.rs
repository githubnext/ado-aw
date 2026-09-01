//! Guard against generated shell regressing to unstructured `format!` bodies.
//!
//! # Why a source-level guard
//!
//! `src/compile/shell/` makes generated shell reviewable and lintable, but
//! nothing stops a future change from going back to
//! `BashStep::new("X", format!("set -eu\n\ …"))`. That would be invisible to
//! both linters: the registry lint only sees registered scripts, and the
//! compiled-YAML lint would flag a *finding* but never the *shape*.
//!
//! The check is deliberately narrow. An earlier survey used the count of
//! `\n\` continuations per file as a proxy for "how much shell is left" and
//! was badly wrong — most of those lines are Rust markdown and error text
//! (`safe_outputs/create_pull_request.rs` has 38 of them and no shell at
//! all). So this does not grep for continuations. It checks one thing that is
//! unambiguous: the script argument of `BashStep::new` must not be built
//! inline.

use std::path::{Path, PathBuf};

fn src_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read src dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Byte offset at which a file's `#[cfg(test)]` module begins, if any.
///
/// Test code legitimately builds throwaway steps inline — the rule is about
/// what the compiler *emits*, not about test fixtures.
fn test_module_start(source: &str) -> usize {
    source
        .find("#[cfg(test)]")
        .unwrap_or(source.len())
}

/// The script argument of a `BashStep::new(name, script)` call starting at
/// `open` (the index of the `(`), or `None` if the call is malformed.
///
/// Balances parentheses and splits on the top-level comma rather than
/// scanning for a terminator: a naive scan overruns the call and picks up
/// unrelated code, and — as the first draft of this guard proved — reports a
/// `format!` in the *display name* of the next call as if it were a shell
/// body.
fn script_argument(source: &str, open: usize) -> Option<&str> {
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut split = None;
    for i in open..bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    let start = split? + 1;
                    return Some(&source[start..i]);
                }
            }
            b',' if depth == 1 && split.is_none() => split = Some(i),
            _ => {}
        }
    }
    None
}

#[test]
fn generated_bash_steps_are_never_built_from_an_inline_format() {
    let mut files = Vec::new();
    rust_sources(&src_dir(), &mut files);
    assert!(!files.is_empty(), "no Rust sources found under src/");

    let mut problems = Vec::new();
    for file in &files {
        // `shell/mod.rs` owns `into_step`, which is the one sanctioned
        // `BashStep::new` in the codebase.
        if file.ends_with(Path::new("compile/shell/mod.rs"))
            || file.ends_with(Path::new("compile\\shell\\mod.rs"))
        {
            continue;
        }
        let source = std::fs::read_to_string(file).expect("read source");
        let production = &source[..test_module_start(&source)];

        const CALL: &str = "BashStep::new";
        for (index, _) in production.match_indices(CALL) {
            let Some(script) = script_argument(production, index + CALL.len()) else {
                continue;
            };
            // A `format!` or an escaped continuation in the *script* argument
            // means the shell was assembled at the call site rather than
            // declared as a registered script. A `format!` in the display
            // name is fine and common.
            if script.contains("format!(") || script.contains("\\n\\") {
                let line = production[..index].lines().count() + 1;
                problems.push(format!(
                    "  {}:{line} builds a bash body inline",
                    file.display()
                ));
            }
        }
    }

    assert!(
        problems.is_empty(),
        "generated shell must be declared with `shell_script!` and rendered \
         through `ShellScript`, not assembled inline. See the \"Generated \
         shell scripts\" section of docs/extending.md.\n{}",
        problems.join("\n")
    );
}

#[test]
fn registered_script_bodies_are_written_verbatim() {
    // A `shell_script!` body is a raw string containing the shell exactly as
    // it runs. A `\n\` continuation inside one means somebody pasted an old
    // `format!` body in without unescaping it, which defeats the point: the
    // body would no longer read as the script that runs.
    let mut files = Vec::new();
    rust_sources(&src_dir(), &mut files);

    let mut problems = Vec::new();
    for file in &files {
        let source = std::fs::read_to_string(file).expect("read source");
        for (index, _) in source.match_indices("shell_script! {") {
            let tail = &source[index..];
            // A body runs to the closing raw-string delimiter.
            let end = tail.find("\"#").map(|e| e + 2).unwrap_or(tail.len());
            if tail[..end].contains("\\n\\") {
                let line = source[..index].lines().count() + 1;
                problems.push(format!(
                    "  {}:{line} has an escaped continuation in a script body",
                    file.display()
                ));
            }
        }
    }

    assert!(
        problems.is_empty(),
        "a `shell_script!` body must be verbatim shell — no `\\n\\` \
         continuations, no escaped quotes:\n{}",
        problems.join("\n")
    );
}

#[test]
fn the_guard_catches_an_inline_body_but_not_a_formatted_display_name() {
    // A guard that only ever passes is indistinguishable from one that does
    // nothing. Exercise the discriminator directly on both shapes.
    const CALL: &str = "BashStep::new";

    let offending = r#"Step::Bash(BashStep::new("Do a thing", format!("set -eu\nrm {p}\n")))"#;
    let index = offending.find(CALL).expect("call present");
    let script = script_argument(offending, index + CALL.len()).expect("argument parsed");
    assert!(
        script.contains("format!("),
        "an inline format! body must be caught, got {script:?}"
    );

    // A `format!` display name with a rendered script is the normal shape for
    // a step that needs extra configuration, and must not be flagged.
    let allowed = r#"BashStep::new(format!("Stage compiler (v{v})"), body)"#;
    let index = allowed.find(CALL).expect("call present");
    let script = script_argument(allowed, index + CALL.len()).expect("argument parsed");
    assert_eq!(script.trim(), "body");
    assert!(!script.contains("format!("));
}

#[test]
fn the_retired_helpers_are_not_reintroduced() {
    // `bash()` and `dedent()` in `agentic_pipeline.rs` existed only to paper
    // over `format!`-built bodies. Re-adding either would signal the pattern
    // is back.
    let path = src_dir().join("compile").join("agentic_pipeline.rs");
    let source = std::fs::read_to_string(&path).expect("read agentic_pipeline.rs");
    for retired in ["\nfn bash(", "\nfn dedent("] {
        assert!(
            !source.contains(retired),
            "`{}` was reintroduced in {}; generated shell should go through \
             `ShellScript` instead",
            retired.trim(),
            path.display()
        );
    }
}
