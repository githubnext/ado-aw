//! The `remove` CLI command.
//!
//! Deletes every ADO build definition that matches a local fixture.
//! Phase 1 of the pipeline-lifecycle CLI family — see `docs/cli.md`.
//!
//! Safety:
//!
//! - Refuses to delete any ADO definition that is not matched against
//!   a local fixture (this falls naturally out of [`match_definitions`]).
//! - Bulk deletes (`> 1` match) require `--yes`. Single-match deletes
//!   require either `--yes` or an interactive `y/N` confirmation on
//!   a tty; non-tty contexts always require `--yes`.

use anyhow::{Context, Result};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use crate::ado::{
    MatchedDefinition, delete_definition, match_definitions, resolve_ado_context, resolve_auth,
};
use crate::detect;

/// Pure decision for the confirm-or-not gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirm {
    /// Proceed without prompting (either `--yes` or `--dry-run`).
    Proceed,
    /// Prompt the operator interactively on the tty before deleting.
    PromptTty,
    /// Bail out — the operator must re-run with `--yes`. The string
    /// is the user-visible reason.
    RequireYes(String),
}

/// Pure function: decide the gating mode for a remove operation.
///
/// Returns [`Confirm::Proceed`] when the caller explicitly opted into
/// the operation, [`Confirm::PromptTty`] for a single match on a tty,
/// or [`Confirm::RequireYes`] when the operator must rerun with
/// `--yes` (bulk deletes, single-match in non-tty).
pub fn decide_confirm(
    match_count: usize,
    yes: bool,
    dry_run: bool,
    is_tty: bool,
) -> Confirm {
    if dry_run || yes {
        return Confirm::Proceed;
    }
    if match_count > 1 {
        return Confirm::RequireYes(format!(
            "{} definitions would be deleted; rerun with --yes to confirm.",
            match_count
        ));
    }
    if match_count == 1 {
        if is_tty {
            return Confirm::PromptTty;
        }
        return Confirm::RequireYes(
            "stdin is not a tty; rerun with --yes to confirm.".to_string(),
        );
    }
    Confirm::Proceed
}

/// CLI options for [`run`].
pub struct RemoveOptions<'a> {
    pub org: Option<&'a str>,
    pub project: Option<&'a str>,
    pub pat: Option<&'a str>,
    pub path: Option<&'a Path>,
    pub yes: bool,
    pub dry_run: bool,
}

/// Run the `remove` command.
pub async fn run(opts: RemoveOptions<'_>) -> Result<()> {
    let repo_path: PathBuf = match opts.path {
        Some(p) => tokio::fs::canonicalize(p)
            .await
            .with_context(|| format!("Could not resolve path: {}", p.display()))?,
        None => tokio::fs::canonicalize(".")
            .await
            .context("Could not resolve current directory")?,
    };

    let auth = resolve_auth(opts.pat).await?;
    let ado_ctx = resolve_ado_context(&repo_path, opts.org, opts.project).await?;

    println!(
        "ADO context: org={}, project={}",
        ado_ctx.org_url, ado_ctx.project
    );
    println!();

    println!("Scanning for agentic workflows...");
    let detected = detect::detect_pipelines(&repo_path).await?;
    if detected.is_empty() {
        // Destructive command: returning Ok(()) here would let a
        // misconfigured invocation (wrong directory, missing
        // `compile`) exit success with no signal that nothing
        // happened. Mirror `disable`'s bail and tell the operator
        // exactly which path was scanned so they can correct it.
        anyhow::bail!(
            "No local agentic workflow fixtures were found under {}. \
             Run `ado-aw compile` first (or point `ado-aw remove` at the repo root). \
             `remove` refuses to exit success in this state because it's destructive.",
            repo_path.display()
        );
    }
    println!("Found {} agentic workflow(s).", detected.len());
    println!();

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("Failed to create HTTP client")?;

    println!("Matching to Azure DevOps pipeline definitions...");
    let matched = match_definitions(&client, &ado_ctx, &auth, &detected).await?;

    if matched.is_empty() {
        anyhow::bail!(
            "No ADO definitions matched any local fixture. Run `ado-aw list` to \
             diagnose; nothing to delete."
        );
    }

    println!("{} definition(s) would be deleted:", matched.len());
    for m in &matched {
        println!("  - {} (id={})", m.name, m.id);
    }
    println!();

    let confirm = decide_confirm(
        matched.len(),
        opts.yes,
        opts.dry_run,
        std::io::stdin().is_terminal(),
    );
    match confirm {
        Confirm::Proceed => {}
        Confirm::RequireYes(reason) => anyhow::bail!("{}", reason),
        Confirm::PromptTty => {
            if !prompt_yes_no(&matched[0])? {
                println!("Aborted by user.");
                return Ok(());
            }
        }
    }

    let mut success = 0usize;
    let mut failure = 0usize;
    for m in &matched {
        if opts.dry_run {
            println!("[dry-run] ✓ would delete: {} (id={})", m.name, m.id);
            success += 1;
            continue;
        }
        match delete_definition(&client, &ado_ctx, &auth, m.id).await {
            Ok(()) => {
                println!("✓ deleted: {} (id={})", m.name, m.id);
                success += 1;
            }
            Err(e) => {
                eprintln!("✗ failed: {} (id={}): {:#}", m.name, m.id, e);
                failure += 1;
            }
        }
    }

    println!();
    println!("Done: {} succeeded, {} failed.", success, failure);
    if failure > 0 {
        anyhow::bail!("{} deletion(s) failed", failure);
    }
    Ok(())
}

fn prompt_yes_no(m: &MatchedDefinition) -> Result<bool> {
    let prompt = format!("Delete '{}' (id={})?", m.name, m.id);
    inquire::Confirm::new(&prompt)
        .with_default(false)
        .prompt()
        .context("Failed to read confirmation from interactive prompt")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============ decide_confirm matrix ============

    #[test]
    fn dry_run_always_proceeds() {
        assert_eq!(decide_confirm(0, false, true, false), Confirm::Proceed);
        assert_eq!(decide_confirm(1, false, true, false), Confirm::Proceed);
        assert_eq!(decide_confirm(5, false, true, false), Confirm::Proceed);
        assert_eq!(decide_confirm(5, false, true, true), Confirm::Proceed);
    }

    #[test]
    fn yes_always_proceeds() {
        assert_eq!(decide_confirm(0, true, false, false), Confirm::Proceed);
        assert_eq!(decide_confirm(1, true, false, false), Confirm::Proceed);
        assert_eq!(decide_confirm(5, true, false, false), Confirm::Proceed);
        assert_eq!(decide_confirm(5, true, false, true), Confirm::Proceed);
    }

    #[test]
    fn bulk_without_yes_requires_yes_even_on_tty() {
        match decide_confirm(3, false, false, true) {
            Confirm::RequireYes(reason) => {
                assert!(reason.contains("3 definitions"), "got: {}", reason);
                assert!(reason.contains("--yes"), "got: {}", reason);
            }
            other => panic!("expected RequireYes, got {:?}", other),
        }
    }

    #[test]
    fn single_match_on_tty_prompts() {
        assert_eq!(decide_confirm(1, false, false, true), Confirm::PromptTty);
    }

    #[test]
    fn single_match_non_tty_requires_yes() {
        match decide_confirm(1, false, false, false) {
            Confirm::RequireYes(reason) => {
                assert!(reason.contains("tty"), "got: {}", reason);
                assert!(reason.contains("--yes"), "got: {}", reason);
            }
            other => panic!("expected RequireYes, got {:?}", other),
        }
    }

    #[test]
    fn zero_matches_proceeds_so_caller_can_handle() {
        // The empty case is handled earlier in `run` (bail with hint)
        // but the gate itself shouldn't block.
        assert_eq!(decide_confirm(0, false, false, true), Confirm::Proceed);
        assert_eq!(decide_confirm(0, false, false, false), Confirm::Proceed);
    }
}
