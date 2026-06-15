//! The `configure` CLI command (deprecated).
//!
//! Sets `GITHUB_TOKEN` on every matched ADO definition. This command
//! is retained as a hidden deprecation alias forwarding to
//! [`crate::secrets::run_set_github_token`]; new code should use
//! `ado-aw secrets set GITHUB_TOKEN <value>` instead.

use anyhow::Result;
use std::path::Path;

/// Forwarder for the legacy `configure --token` invocation. Emits a
/// deprecation warning to stderr and forwards to the unified
/// `secrets set GITHUB_TOKEN` code path.
pub async fn run(
    token: Option<&str>,
    org: Option<&str>,
    project: Option<&str>,
    pat: Option<&str>,
    path: Option<&Path>,
    dry_run: bool,
    definition_ids: Option<&[u64]>,
) -> Result<()> {
    crate::secrets::run_set_github_token(token, org, project, pat, path, dry_run, definition_ids)
        .await
}
