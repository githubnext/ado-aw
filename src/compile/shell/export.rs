//! Materialise every registered shell script for review and analysis.
//!
//! Backs `ado-aw export-bash-scripts`. The registry already makes the set of
//! scripts enumerable in-process; this makes it enumerable *outside* the
//! process, so a reviewer, an agentic workflow, or any shell-analysis tool can
//! work on the scripts as ordinary files rather than by reading Rust.
//!
//! Two forms, because they answer different questions:
//!
//! * `--format files` (default) writes one `.sh` per script with a provenance
//!   header. This is what you run `shellcheck`, `shfmt` or a diff over.
//! * `--format json` writes a single document carrying the same content plus
//!   the declared binding surface, for tooling that wants structure.
//!
//! What is written is [`ShellScriptDef::lint_source`] — the body with declared
//! variables stub-assigned — not a rendered script. A rendered script needs
//! real bindings, which only the producing call site has; the lint source is
//! the form that stands alone and is what the shellcheck harness judges.

use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

use super::registry::all_scripts;

/// Output shape for `ado-aw export-bash-scripts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ExportFormat {
    /// One `.sh` file per script, with a provenance header.
    Files,
    /// A single JSON document describing every script.
    Json,
}

/// One script as exported.
#[derive(Debug, Serialize)]
struct ExportedScript {
    name: String,
    interpreter: &'static str,
    file: &'static str,
    line: u32,
    bindings: &'static [&'static str],
    externals: &'static [&'static str],
    fragments: &'static [&'static str],
    source: String,
}

/// Write every registered script to `out_dir`.
///
/// Returns the number of scripts exported.
pub fn export(out_dir: &Path, format: ExportFormat) -> Result<usize> {
    let scripts = all_scripts();
    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("creating export directory {}", out_dir.display()))?;

    let exported: Vec<ExportedScript> = scripts
        .iter()
        .map(|def| ExportedScript {
            name: def.name.to_string(),
            interpreter: def.interpreter.shellcheck_dialect(),
            file: def.file,
            line: def.line,
            bindings: def.bindings,
            externals: def.externals,
            fragments: def.fragments,
            source: def.lint_source(),
        })
        .collect();

    match format {
        ExportFormat::Json => {
            let path = out_dir.join("bash-scripts.json");
            let json = serde_json::to_string_pretty(&exported)?;
            std::fs::write(&path, json)
                .with_context(|| format!("writing {}", path.display()))?;
        }
        ExportFormat::Files => {
            for (def, script) in scripts.iter().zip(&exported) {
                let path = out_dir.join(def.export_file_name());
                let contents = format!("{}{}", provenance_header(script), script.source);
                std::fs::write(&path, contents)
                    .with_context(|| format!("writing {}", path.display()))?;
            }
        }
    }

    Ok(exported.len())
}

/// A header that points a reader back at the producing Rust source, so a
/// finding in an exported file is actionable without a repository-wide grep.
fn provenance_header(script: &ExportedScript) -> String {
    format!(
        "# ado-aw generated export — do not edit.\n\
         # script:      {}\n\
         # source:      {}:{}\n\
         # interpreter: {}\n\
         # bindings:    {}\n\
         # externals:   {}\n\
         #\n\
         # Variables below the lint-stub marker are placeholders. The real\n\
         # values are bound at the call site in the source file above.\n",
        script.name,
        script.file,
        script.line,
        script.interpreter,
        join_or_none(script.bindings),
        join_or_none(script.externals),
    )
}

fn join_or_none(values: &[&str]) -> String {
    if values.is_empty() {
        "(none)".to_string()
    } else {
        values.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell_script;

    shell_script! {
        /// Fixture for export behaviour.
        EXPORT_FIXTURE {
            interpreter: Bash,
            bindings: [PROXY_CONTAINER],
            externals: [],
            fragments: [],
            body: r#"
docker rm -f "$PROXY_CONTAINER" 2>/dev/null || true
"#,
        }
    }

    #[test]
    fn files_mode_writes_one_script_per_registration() {
        let dir = tempfile::tempdir().expect("temp dir");
        let count = export(dir.path(), ExportFormat::Files).expect("export");
        assert_eq!(count, all_scripts().len());
        assert!(count > 0, "the registry must not be empty");

        let path = dir.path().join(EXPORT_FIXTURE.export_file_name());
        let contents = std::fs::read_to_string(&path).expect("read exported script");
        // Provenance first, so a finding is traceable without a grep.
        assert!(contents.contains("# source:      src"));
        assert!(contents.contains("# bindings:    PROXY_CONTAINER"));
        // Then the shell itself, verbatim and unescaped.
        assert!(contents.contains("docker rm -f \"$PROXY_CONTAINER\" 2>/dev/null || true"));
    }

    #[test]
    fn files_mode_creates_a_missing_directory() {
        let dir = tempfile::tempdir().expect("temp dir");
        let nested = dir.path().join("a").join("b");
        export(&nested, ExportFormat::Files).expect("export into a fresh path");
        assert!(nested.join(EXPORT_FIXTURE.export_file_name()).exists());
    }

    #[test]
    fn json_mode_carries_the_declared_surface() {
        let dir = tempfile::tempdir().expect("temp dir");
        export(dir.path(), ExportFormat::Json).expect("export");
        let raw = std::fs::read_to_string(dir.path().join("bash-scripts.json")).expect("read");
        let parsed: serde_json::Value = serde_json::from_str(&raw).expect("valid JSON");
        let entry = parsed
            .as_array()
            .expect("array")
            .iter()
            .find(|e| e["name"].as_str().unwrap_or_default().ends_with("::EXPORT_FIXTURE"))
            .expect("the fixture is exported");
        assert_eq!(entry["interpreter"], "bash");
        assert_eq!(entry["bindings"][0], "PROXY_CONTAINER");
        assert_eq!(entry["externals"].as_array().expect("array").len(), 0);
    }

    #[test]
    fn empty_declarations_render_as_none_rather_than_blank() {
        assert_eq!(join_or_none(&[]), "(none)");
        assert_eq!(join_or_none(&["A", "B"]), "A, B");
    }
}
