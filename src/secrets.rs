//! The `secrets` CLI command (subcommand group).
//!
//! Replaces `ado-aw configure` with a `secrets set / list / delete`
//! subcommand group. `secrets set GITHUB_TOKEN <value>` is the direct
//! replacement for `configure --token <value>`; the legacy
//! `configure` invocation is still accepted (hidden in `--help`) and
//! prints a deprecation warning before forwarding to
//! [`run_set_github_token`].
//!
//! Phase 1 of the pipeline-lifecycle CLI family — see `docs/cli.md`.
//!
//! ## Security
//!
//! - `secrets list` never prints variable values. It only emits names
//!   and the `isSecret` / `allowOverride` flags.
//! - `secrets set` PUTs with `isSecret: true`.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use crate::ado::{
    AdoAuth, AdoContext, MatchedDefinition, PATH_SEGMENT, get_definition_full,
    resolve_ado_context, resolve_auth, resolve_definitions,
};

/// Description of one pipeline variable, for listing only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariableInfo {
    pub name: String,
    pub is_secret: bool,
    pub allow_override: bool,
}

/// Validate a variable name. ADO permits arbitrary names but the CLI
/// rejects empty/whitespace-only/whitespace-containing inputs since
/// those almost always indicate a quoting bug.
pub fn validate_variable_name(name: &str) -> Result<()> {
    if name.is_empty() {
        anyhow::bail!("Variable name must not be empty.");
    }
    if name.chars().any(char::is_whitespace) {
        anyhow::bail!(
            "Variable name '{}' contains whitespace; check shell quoting.",
            name
        );
    }
    Ok(())
}

/// Pure: produce a copy of `definition` with the named variable set
/// to `(value, isSecret=true, allow_override)`. Preserves all other
/// keys.
pub fn apply_variable_set(
    mut definition: serde_json::Value,
    name: &str,
    value: &str,
    allow_override: bool,
) -> serde_json::Value {
    if definition.get("variables").is_none()
        || !definition["variables"].is_object()
    {
        definition["variables"] = serde_json::json!({});
    }
    definition["variables"][name] = serde_json::json!({
        "value": value,
        "isSecret": true,
        "allowOverride": allow_override,
    });
    definition
}

/// Pure: produce a copy of `definition` with the named variable
/// removed. No-op if it wasn't present.
pub fn apply_variable_delete(
    mut definition: serde_json::Value,
    name: &str,
) -> serde_json::Value {
    if let Some(vars) = definition.get_mut("variables").and_then(|v| v.as_object_mut()) {
        vars.remove(name);
    }
    definition
}

/// Pure: project a definition's `variables` object to a sorted
/// list of [`VariableInfo`]. Never reads or surfaces the `value`
/// field — listings must be safe to dump to stdout.
pub fn list_variables(definition: &serde_json::Value) -> Vec<VariableInfo> {
    let Some(vars) = definition.get("variables").and_then(|v| v.as_object()) else {
        return Vec::new();
    };
    let mut out: Vec<VariableInfo> = vars
        .iter()
        .map(|(k, v)| VariableInfo {
            name: k.clone(),
            is_secret: v.get("isSecret").and_then(|b| b.as_bool()).unwrap_or(false),
            allow_override: v
                .get("allowOverride")
                .and_then(|b| b.as_bool())
                .unwrap_or(false),
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

// ==================== Shared HTTP helpers ====================

async fn put_definition(
    client: &reqwest::Client,
    ctx: &AdoContext,
    auth: &AdoAuth,
    id: u64,
    body: &serde_json::Value,
) -> Result<()> {
    let url = format!(
        "{}/{}/_apis/build/definitions/{}?api-version=7.1",
        ctx.org_url.trim_end_matches('/'),
        percent_encoding::utf8_percent_encode(&ctx.project, PATH_SEGMENT),
        id
    );

    let resp = auth
        .apply(client.put(&url))
        .header("Content-Type", "application/json")
        .json(body)
        .send()
        .await
        .with_context(|| format!("Failed to PUT definition {}", id))?;

    let status = resp.status();
    if !status.is_success() {
        let resp_body = resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "ADO API returned {} when PUTting definition {}: {}",
            status,
            id,
            resp_body
        );
    }
    Ok(())
}

// ==================== `secrets set` ====================

pub struct SetOptions<'a> {
    pub name: &'a str,
    pub value: Option<&'a str>,
    pub org: Option<&'a str>,
    pub project: Option<&'a str>,
    pub pat: Option<&'a str>,
    pub path: Option<&'a Path>,
    pub allow_override: bool,
    pub value_stdin: bool,
    pub dry_run: bool,
    pub definition_ids: Option<&'a [u64]>,
}

pub async fn run_set(opts: SetOptions<'_>) -> Result<()> {
    validate_variable_name(opts.name)?;

    let repo_path: PathBuf = match opts.path {
        Some(p) => tokio::fs::canonicalize(p)
            .await
            .with_context(|| format!("Could not resolve path: {}", p.display()))?,
        None => tokio::fs::canonicalize(".")
            .await
            .context("Could not resolve current directory")?,
    };

    let value = resolve_value(opts.name, opts.value, opts.value_stdin)?;
    let auth = resolve_auth(opts.pat).await?;
    let ado_ctx = resolve_ado_context(&repo_path, opts.org, opts.project).await?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("Failed to create HTTP client")?;

    let Some(matched) = resolve_definitions(
        &client,
        &ado_ctx,
        &auth,
        opts.definition_ids,
        &repo_path,
    )
    .await?
    else {
        return Ok(());
    };

    if matched.is_empty() {
        anyhow::bail!(
            "No ADO definitions matched any local fixture. Run `ado-aw list` to \
             diagnose."
        );
    }

    print_matched_summary(&matched);

    if opts.dry_run {
        println!(
            "[dry-run] Would set '{}' (isSecret=true, allowOverride={}) on {} definition(s).",
            opts.name,
            opts.allow_override,
            matched.len()
        );
        return Ok(());
    }

    let mut success = 0usize;
    let mut failure = 0usize;
    for m in &matched {
        match apply_set_one(
            &client,
            &ado_ctx,
            &auth,
            m.id,
            opts.name,
            &value,
            opts.allow_override,
        )
        .await
        {
            Ok(()) => {
                println!("  ✓ '{}' set on '{}' (id={})", opts.name, m.name, m.id);
                success += 1;
            }
            Err(e) => {
                eprintln!("  ✗ '{}' on '{}' (id={}): {:#}", opts.name, m.name, m.id, e);
                failure += 1;
            }
        }
    }

    println!();
    println!("Done: {} succeeded, {} failed.", success, failure);
    if failure > 0 {
        anyhow::bail!("{} definition(s) failed", failure);
    }
    Ok(())
}

async fn apply_set_one(
    client: &reqwest::Client,
    ctx: &AdoContext,
    auth: &AdoAuth,
    id: u64,
    name: &str,
    value: &str,
    allow_override: bool,
) -> Result<()> {
    let definition = get_definition_full(client, ctx, auth, id).await?;
    let updated = apply_variable_set(definition, name, value, allow_override);
    put_definition(client, ctx, auth, id, &updated).await
}

/// Resolve the variable value from the CLI inputs: explicit positional
/// `value` first, then `--value-stdin` (reads exactly one line), then
/// an interactive tty prompt with echo off.
fn resolve_value(
    name: &str,
    explicit: Option<&str>,
    value_stdin: bool,
) -> Result<String> {
    if let Some(v) = explicit {
        return Ok(v.to_string());
    }
    if value_stdin {
        use std::io::BufRead;
        let mut line = String::new();
        let stdin = std::io::stdin();
        stdin.lock().read_line(&mut line).context("Failed to read value from stdin")?;
        let trimmed = line.trim_end_matches(['\r', '\n']).to_string();
        if trimmed.is_empty() {
            anyhow::bail!("--value-stdin read an empty value");
        }
        return Ok(trimmed);
    }
    inquire::Password::new(&format!("Enter value for {}:", name))
        .without_confirmation()
        .prompt()
        .context("Failed to read value from interactive prompt")
}

// ==================== `secrets list` ====================

pub struct ListOptions<'a> {
    pub org: Option<&'a str>,
    pub project: Option<&'a str>,
    pub pat: Option<&'a str>,
    pub path: Option<&'a Path>,
    pub json: bool,
    pub definition_ids: Option<&'a [u64]>,
}

pub async fn run_list(opts: ListOptions<'_>) -> Result<()> {
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

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("Failed to create HTTP client")?;

    let Some(matched) = resolve_definitions(
        &client,
        &ado_ctx,
        &auth,
        opts.definition_ids,
        &repo_path,
    )
    .await?
    else {
        return Ok(());
    };

    if matched.is_empty() {
        anyhow::bail!(
            "No ADO definitions matched any local fixture. Run `ado-aw list` to \
             diagnose."
        );
    }

    let mut payload = serde_json::json!({});
    for m in &matched {
        let definition = get_definition_full(&client, &ado_ctx, &auth, m.id).await?;
        let vars = list_variables(&definition);

        if opts.json {
            payload[m.id.to_string()] = serde_json::json!({
                "name": m.name,
                "variables": vars.iter().map(|v| serde_json::json!({
                    "name": v.name,
                    "isSecret": v.is_secret,
                    "allowOverride": v.allow_override,
                })).collect::<Vec<_>>(),
            });
        } else {
            println!("● {} (id={})", m.name, m.id);
            if vars.is_empty() {
                println!("  (no variables)");
            } else {
                for v in &vars {
                    println!(
                        "  - {}  isSecret={}  allowOverride={}",
                        v.name, v.is_secret, v.allow_override
                    );
                }
            }
            println!();
        }
    }

    if opts.json {
        println!("{}", serde_json::to_string_pretty(&payload)?);
    }
    Ok(())
}

// ==================== `secrets delete` ====================

pub struct DeleteOptions<'a> {
    pub name: &'a str,
    pub org: Option<&'a str>,
    pub project: Option<&'a str>,
    pub pat: Option<&'a str>,
    pub path: Option<&'a Path>,
    pub dry_run: bool,
    pub definition_ids: Option<&'a [u64]>,
}

pub async fn run_delete(opts: DeleteOptions<'_>) -> Result<()> {
    validate_variable_name(opts.name)?;

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

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("Failed to create HTTP client")?;

    let Some(matched) = resolve_definitions(
        &client,
        &ado_ctx,
        &auth,
        opts.definition_ids,
        &repo_path,
    )
    .await?
    else {
        return Ok(());
    };

    if matched.is_empty() {
        anyhow::bail!(
            "No ADO definitions matched any local fixture. Run `ado-aw list` to \
             diagnose."
        );
    }

    print_matched_summary(&matched);

    if opts.dry_run {
        println!(
            "[dry-run] Would delete variable '{}' from {} definition(s) (no-op when absent).",
            opts.name,
            matched.len()
        );
        return Ok(());
    }

    let mut success = 0usize;
    let mut failure = 0usize;
    for m in &matched {
        match apply_delete_one(&client, &ado_ctx, &auth, m.id, opts.name).await {
            Ok(()) => {
                println!("  ✓ '{}' removed from '{}' (id={})", opts.name, m.name, m.id);
                success += 1;
            }
            Err(e) => {
                eprintln!(
                    "  ✗ removing '{}' from '{}' (id={}): {:#}",
                    opts.name, m.name, m.id, e
                );
                failure += 1;
            }
        }
    }

    println!();
    println!("Done: {} succeeded, {} failed.", success, failure);
    if failure > 0 {
        anyhow::bail!("{} definition(s) failed", failure);
    }
    Ok(())
}

async fn apply_delete_one(
    client: &reqwest::Client,
    ctx: &AdoContext,
    auth: &AdoAuth,
    id: u64,
    name: &str,
) -> Result<()> {
    let definition = get_definition_full(client, ctx, auth, id).await?;
    let updated = apply_variable_delete(definition, name);
    put_definition(client, ctx, auth, id, &updated).await
}

// ==================== Deprecation alias ====================

/// Shim for the legacy `configure --token` invocation. Sets
/// `GITHUB_TOKEN` (isSecret=true, allowOverride preserved) on every
/// matched definition. Same fail-soft + accumulated-counts pattern as
/// the old `configure` body, lifted here verbatim so the deprecation
/// alias is byte-equivalent.
pub async fn run_set_github_token(
    token: Option<&str>,
    org: Option<&str>,
    project: Option<&str>,
    pat: Option<&str>,
    path: Option<&Path>,
    dry_run: bool,
    definition_ids: Option<&[u64]>,
) -> Result<()> {
    eprintln!(
        "warning: 'ado-aw configure' is deprecated; use 'ado-aw secrets set GITHUB_TOKEN' \
         instead. The alias will be removed in the next minor release."
    );
    run_set(SetOptions {
        name: "GITHUB_TOKEN",
        value: token,
        org,
        project,
        pat,
        path,
        allow_override: false,
        value_stdin: false,
        dry_run,
        definition_ids,
    })
    .await
}

// ==================== Shared display helpers ====================

fn print_matched_summary(matched: &[MatchedDefinition]) {
    println!("{} definition(s) matched:", matched.len());
    for m in matched {
        if m.yaml_path.is_empty() {
            println!("  [{}] '{}' (id={})", m.match_method, m.name, m.id);
        } else {
            println!(
                "  [{}] '{}' (id={}) ← {}",
                m.match_method, m.name, m.id, m.yaml_path
            );
        }
    }
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============ validate_variable_name ============

    #[test]
    fn validate_rejects_empty() {
        assert!(validate_variable_name("").is_err());
    }

    #[test]
    fn validate_rejects_whitespace() {
        assert!(validate_variable_name("   ").is_err());
        assert!(validate_variable_name("FOO BAR").is_err());
        assert!(validate_variable_name("FOO\tBAR").is_err());
    }

    #[test]
    fn validate_accepts_typical_names() {
        assert!(validate_variable_name("GITHUB_TOKEN").is_ok());
        assert!(validate_variable_name("MY-VAR").is_ok());
        assert!(validate_variable_name("a.b.c").is_ok());
    }

    // ============ apply_variable_set ============

    #[test]
    fn set_creates_variables_object_when_missing() {
        let def = serde_json::json!({ "name": "x" });
        let out = apply_variable_set(def, "FOO", "bar", false);
        assert_eq!(out["variables"]["FOO"]["value"], "bar");
        assert_eq!(out["variables"]["FOO"]["isSecret"], true);
        assert_eq!(out["variables"]["FOO"]["allowOverride"], false);
    }

    #[test]
    fn set_preserves_other_variables() {
        let def = serde_json::json!({
            "variables": { "OTHER": { "value": "x", "isSecret": true, "allowOverride": false } }
        });
        let out = apply_variable_set(def, "FOO", "bar", true);
        assert_eq!(out["variables"]["OTHER"]["value"], "x");
        assert_eq!(out["variables"]["FOO"]["value"], "bar");
        assert_eq!(out["variables"]["FOO"]["allowOverride"], true);
    }

    #[test]
    fn set_overwrites_existing_variable() {
        let def = serde_json::json!({
            "variables": { "FOO": { "value": "old", "isSecret": true, "allowOverride": false } }
        });
        let out = apply_variable_set(def, "FOO", "new", true);
        assert_eq!(out["variables"]["FOO"]["value"], "new");
        assert_eq!(out["variables"]["FOO"]["allowOverride"], true);
    }

    // ============ apply_variable_delete ============

    #[test]
    fn delete_removes_existing_variable() {
        let def = serde_json::json!({
            "variables": {
                "FOO": { "value": "v" },
                "BAR": { "value": "w" }
            }
        });
        let out = apply_variable_delete(def, "FOO");
        assert!(out["variables"].get("FOO").is_none());
        assert_eq!(out["variables"]["BAR"]["value"], "w");
    }

    #[test]
    fn delete_is_noop_when_variable_absent() {
        let def = serde_json::json!({ "variables": { "FOO": { "value": "v" } } });
        let out = apply_variable_delete(def, "MISSING");
        assert_eq!(out["variables"]["FOO"]["value"], "v");
    }

    #[test]
    fn delete_is_noop_when_variables_object_missing() {
        let def = serde_json::json!({ "name": "x" });
        let out = apply_variable_delete(def, "MISSING");
        assert!(out.get("variables").is_none() || out["variables"].is_null());
    }

    // ============ list_variables (no values surfaced) ============

    #[test]
    fn list_emits_names_and_flags_only() {
        let def = serde_json::json!({
            "variables": {
                "TOKEN": { "value": "super-secret-leak-me", "isSecret": true, "allowOverride": false },
                "DEBUG": { "value": "true", "isSecret": false, "allowOverride": true }
            }
        });
        let out = list_variables(&def);
        assert_eq!(out.len(), 2);
        // Sorted by name.
        assert_eq!(out[0].name, "DEBUG");
        assert!(!out[0].is_secret);
        assert!(out[0].allow_override);
        assert_eq!(out[1].name, "TOKEN");
        assert!(out[1].is_secret);
        assert!(!out[1].allow_override);
    }

    #[test]
    fn list_returns_empty_when_no_variables_object() {
        let def = serde_json::json!({ "name": "x" });
        assert!(list_variables(&def).is_empty());
    }

    /// Guardrail: the VariableInfo struct has no `value` field. If you
    /// ever feel tempted to add one, you'll need to change the
    /// printer — and ideally have a different review reason than
    /// "convenience".
    #[test]
    fn variable_info_has_no_value_field_in_debug_repr() {
        let def = serde_json::json!({
            "variables": {
                "TOKEN": { "value": "super-secret", "isSecret": true, "allowOverride": false }
            }
        });
        let out = list_variables(&def);
        let dbg = format!("{:?}", out[0]);
        assert!(
            !dbg.contains("super-secret"),
            "VariableInfo Debug must not leak values, got: {}",
            dbg
        );
    }
}
