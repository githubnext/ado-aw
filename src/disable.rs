//! The `disable` CLI command.
//!
//! Sets `queueStatus` to `disabled` (default) or `paused` on every ADO
//! build definition matched against a local fixture. Phase 1 of the
//! pipeline-lifecycle CLI family — see `docs/cli.md`.
//!
//! Scope (Phase 1):
//!
//! - Only touches ADO definitions that map to a local fixture. This
//!   safety property falls naturally out of [`match_definitions`] —
//!   definitions without a local fixture are never in the returned
//!   set.
//! - No-op (skip) when the current `queueStatus` already matches the
//!   target.

use anyhow::{Context, Result};
use log::debug;
use std::path::{Path, PathBuf};

use crate::ado::{
    AdoAuth, AdoContext, MatchedDefinition, match_definitions, patch_queue_status,
    resolve_ado_context, resolve_auth,
};
use crate::detect;

/// Which `queueStatus` value the operator wants to land on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Disabled,
    Paused,
}

impl Target {
    pub fn as_str(&self) -> &'static str {
        match self {
            Target::Disabled => "disabled",
            Target::Paused => "paused",
        }
    }
}

/// Outcome of inspecting one matched definition against the operator's
/// requested target.
///
/// Pure data — no HTTP, no auth, no IO. Built by [`decide_action`] so
/// the decision logic can be exercised without touching the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Already at the requested status; nothing to do.
    Skip {
        id: u64,
        name: String,
        reason: String,
    },
    /// `queueStatus` needs to be patched.
    Patch {
        id: u64,
        name: String,
        from: String,
        to: &'static str,
    },
}

/// Pure function: decide what to do for one matched definition.
///
/// - `Skip` when the current status equals the target.
/// - `Patch` otherwise — including the "current status is unknown"
///   case (older API responses, explicit-ID matches), since the safe
///   default is to apply the patch and let ADO reject if appropriate.
pub fn decide_action(matched: &MatchedDefinition, target: Target) -> Action {
    let target_str = target.as_str();
    let current = matched.queue_status.as_deref().unwrap_or("");

    if current == target_str {
        return Action::Skip {
            id: matched.id,
            name: matched.name.clone(),
            reason: format!("already {}", target_str),
        };
    }

    Action::Patch {
        id: matched.id,
        name: matched.name.clone(),
        from: if current.is_empty() {
            "unknown".to_string()
        } else {
            current.to_string()
        },
        to: target_str,
    }
}

/// CLI options for [`run`].
pub struct DisableOptions<'a> {
    pub org: Option<&'a str>,
    pub project: Option<&'a str>,
    pub pat: Option<&'a str>,
    pub path: Option<&'a Path>,
    pub paused: bool,
    pub dry_run: bool,
}

/// Outcome of applying one [`Action`] (used to tally the final summary).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApplyOutcome {
    Skipped,
    Patched,
    Failed,
}

/// Apply a single action against ADO, print a one-line status, and return the
/// outcome so the caller can tally results without nesting.
async fn apply_action(
    action: Action,
    client: &reqwest::Client,
    ado_ctx: &AdoContext,
    auth: &AdoAuth,
    dry_run: bool,
) -> ApplyOutcome {
    match action {
        Action::Skip { id, name, reason } => {
            println!("↻ skip: {} (id={}, {})", name, id, reason);
            ApplyOutcome::Skipped
        }
        Action::Patch { id, name, from, to } => {
            if dry_run {
                println!(
                    "[dry-run] ▶ would patch: {} (id={}, {} → {})",
                    name, id, from, to
                );
                return ApplyOutcome::Patched;
            }
            match patch_queue_status(client, ado_ctx, auth, id, to).await {
                Ok(()) => {
                    println!("▶ patched: {} (id={}, {} → {})", name, id, from, to);
                    ApplyOutcome::Patched
                }
                Err(e) => {
                    eprintln!("✗ failed: {} (id={}): {:#}", name, id, e);
                    ApplyOutcome::Failed
                }
            }
        }
    }
}

/// Run the `disable` command.
pub async fn run(opts: DisableOptions<'_>) -> Result<()> {
    let repo_path: PathBuf = match opts.path {
        Some(p) => tokio::fs::canonicalize(p)
            .await
            .with_context(|| format!("Could not resolve path: {}", p.display()))?,
        None => tokio::fs::canonicalize(".")
            .await
            .context("Could not resolve current directory")?,
    };

    let target = if opts.paused {
        Target::Paused
    } else {
        Target::Disabled
    };

    let auth = resolve_auth(opts.pat).await?;
    let ado_ctx = resolve_ado_context(&repo_path, opts.org, opts.project).await?;

    println!(
        "ADO context: org={}, project={}",
        ado_ctx.org_url, ado_ctx.project
    );
    println!("Target queueStatus: {}", target.as_str());
    println!();

    println!("Scanning for agentic pipelines...");
    let detected = detect::detect_pipelines(&repo_path).await?;
    if detected.is_empty() {
        anyhow::bail!(
            "No local agentic pipeline fixtures were found under {}. \
             Run `ado-aw compile` first (or point `ado-aw disable` at the repo root).",
            repo_path.display()
        );
    }
    println!("Found {} agentic pipeline(s).", detected.len());
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
             diagnose: either the fixtures haven't been registered with `ado-aw \
             enable`, or the local yaml paths and ADO `yamlFilename` values don't \
             line up."
        );
    }

    println!("{} definition(s) matched.", matched.len());
    println!();

    let mut patched = 0usize;
    let mut skipped = 0usize;
    let mut failure = 0usize;
    for m in &matched {
        let action = decide_action(m, target);
        debug!("definition {}: action={:?}", m.id, action);
        match apply_action(action, &client, &ado_ctx, &auth, opts.dry_run).await {
            ApplyOutcome::Skipped => skipped += 1,
            ApplyOutcome::Patched => patched += 1,
            ApplyOutcome::Failed => failure += 1,
        }
    }

    println!();
    println!(
        "Done: {} patched, {} skipped, {} failed.",
        patched, skipped, failure
    );
    if failure > 0 {
        anyhow::bail!("{} definition(s) failed", failure);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ado::MatchMethod;

    fn matched_with_status(id: u64, name: &str, status: Option<&str>) -> MatchedDefinition {
        MatchedDefinition {
            id,
            name: name.to_string(),
            match_method: MatchMethod::YamlPath,
            yaml_path: format!("/tests/{}.lock.yml", name.replace(' ', "-")),
            queue_status: status.map(str::to_string),
        }
    }

    #[test]
    fn target_as_str_disabled_and_paused() {
        assert_eq!(Target::Disabled.as_str(), "disabled");
        assert_eq!(Target::Paused.as_str(), "paused");
    }

    // ============ decide_action matrix ============

    #[test]
    fn enabled_to_disabled_patches() {
        let m = matched_with_status(1, "noop", Some("enabled"));
        let action = decide_action(&m, Target::Disabled);
        assert_eq!(
            action,
            Action::Patch {
                id: 1,
                name: "noop".to_string(),
                from: "enabled".to_string(),
                to: "disabled"
            }
        );
    }

    #[test]
    fn enabled_to_paused_patches() {
        let m = matched_with_status(2, "noop", Some("enabled"));
        let action = decide_action(&m, Target::Paused);
        assert_eq!(
            action,
            Action::Patch {
                id: 2,
                name: "noop".to_string(),
                from: "enabled".to_string(),
                to: "paused"
            }
        );
    }

    #[test]
    fn disabled_to_disabled_skips() {
        let m = matched_with_status(3, "noop", Some("disabled"));
        let action = decide_action(&m, Target::Disabled);
        assert_eq!(
            action,
            Action::Skip {
                id: 3,
                name: "noop".to_string(),
                reason: "already disabled".to_string()
            }
        );
    }

    #[test]
    fn paused_to_paused_skips() {
        let m = matched_with_status(4, "noop", Some("paused"));
        let action = decide_action(&m, Target::Paused);
        assert_eq!(
            action,
            Action::Skip {
                id: 4,
                name: "noop".to_string(),
                reason: "already paused".to_string()
            }
        );
    }

    #[test]
    fn disabled_to_paused_patches() {
        let m = matched_with_status(5, "noop", Some("disabled"));
        let action = decide_action(&m, Target::Paused);
        assert_eq!(
            action,
            Action::Patch {
                id: 5,
                name: "noop".to_string(),
                from: "disabled".to_string(),
                to: "paused"
            }
        );
    }

    #[test]
    fn paused_to_disabled_patches() {
        let m = matched_with_status(6, "noop", Some("paused"));
        let action = decide_action(&m, Target::Disabled);
        assert_eq!(
            action,
            Action::Patch {
                id: 6,
                name: "noop".to_string(),
                from: "paused".to_string(),
                to: "disabled"
            }
        );
    }

    #[test]
    fn unknown_status_patches_with_from_unknown() {
        // Explicit-ID matches and older API responses have queue_status = None.
        // The safe default is to apply the patch and let ADO reject if needed.
        let m = matched_with_status(7, "noop", None);
        let action = decide_action(&m, Target::Disabled);
        assert_eq!(
            action,
            Action::Patch {
                id: 7,
                name: "noop".to_string(),
                from: "unknown".to_string(),
                to: "disabled"
            }
        );
    }

    #[test]
    fn empty_status_string_treated_as_unknown() {
        let m = matched_with_status(8, "noop", Some(""));
        let action = decide_action(&m, Target::Disabled);
        match action {
            Action::Patch { from, .. } => assert_eq!(from, "unknown"),
            other => panic!("expected Patch, got {:?}", other),
        }
    }
}
