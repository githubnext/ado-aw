//! The `run` CLI command.
//!
//! Queues a build for every ADO definition that matches a local
//! fixture. With `--wait`, polls each queued build until completion
//! and exits with a status code that reflects the aggregate result.
//! Phase 1 of the pipeline-lifecycle CLI family — see `docs/cli.md`.
//!
//! Naming nit: the module-level entry point is `dispatch`, not `run`,
//! so call sites don't end up reading `run::run(...)`. Don't rename
//! it back to `run` — future contributors will find this comment if
//! they try.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::ado::{
    AdoAuth, AdoContext, MatchedDefinition, PATH_SEGMENT, get_build, match_definitions,
    queue_build, resolve_ado_context, resolve_auth,
};
use crate::detect;

/// Parse `--parameters foo=bar,baz=qux` (and its repeatable form) into
/// a JSON map. Pure function; reject malformed pairs.
///
/// **Values must not contain commas.** Each raw argument is split on
/// `,` *before* the `=` split, so a value like `redirect_uri=https://a,b`
/// is torn into two pairs and the trailing fragment (`b`) is rejected
/// because it has no `=`.
///
/// There is currently no way to escape a comma inside a single
/// `--parameters` argument. The CLI also splits any single argument
/// on `,`, so passing the comma-containing value as a separate flag
/// does **not** help either — it's the comma in the argument value
/// (not the argument boundary) that matters.
///
/// - ❌ `--parameters key=a,b`
///   → splits to `key=a` + `b`; the second pair fails with `no '='`.
/// - ❌ `--parameters 'urls=a,b' --parameters mode=fast`
///   → same split happens inside the first argument; the result is
///   `key=urls=a` + `b` + `mode=fast` and the `b` fragment is rejected.
/// - ✅ `--parameters mode=fast --parameters extra=x`
///   → one pair per flag, no commas in values; both pairs parse.
///
/// If you need to pass a comma in a value, the only workaround today is
/// to write the value without the comma (e.g. URL-encode it on the
/// caller side and have the pipeline decode it). A follow-up could add
/// escape syntax (`--parameters 'urls=a\,b'`) without breaking this
/// rule.
///
/// Only the first `=` in a pair is treated as the separator; subsequent
/// `=` characters are part of the value, so `key=a=b=c` parses as
/// `{"key": "a=b=c"}`.
pub fn parse_parameters(values: &[String]) -> Result<serde_json::Map<String, serde_json::Value>> {
    let mut out = serde_json::Map::new();
    for raw in values {
        // The argument-level comma split makes values containing
        // commas impossible to express today. Detect the
        // ambiguous-fragment case (a comma in the raw argument and
        // a fragment with no `=`) and produce a self-diagnosable
        // hint instead of the bare "no '=' found" error.
        let raw_has_comma = raw.contains(',');
        for pair in raw.split(',') {
            let pair = pair.trim();
            if pair.is_empty() {
                continue;
            }
            let Some((k, v)) = pair.split_once('=') else {
                if raw_has_comma {
                    anyhow::bail!(
                        "Invalid --parameters pair '{}': expected key=value (no '=' found). \
                         Hint: values must not contain commas. The raw argument '{}' was \
                         split on ',' before the '=' split; use a separate --parameters flag \
                         per pair.",
                        pair,
                        raw
                    );
                }
                anyhow::bail!(
                    "Invalid --parameters pair '{}': expected key=value (no '=' found).",
                    pair
                );
            };
            let key = k.trim();
            if key.is_empty() {
                anyhow::bail!("Invalid --parameters pair '{}': empty key.", pair);
            }
            // All values are strings — ADO coerces template-parameter
            // values as the pipeline definition requires.
            out.insert(key.to_string(), serde_json::Value::String(v.trim().to_string()));
        }
    }
    Ok(out)
}

/// Build a `(definition_id, queued_build_id)` poll-target pair.
#[derive(Debug, Clone, Copy)]
struct PollTarget {
    definition_id: u64,
    build_id: u64,
}

/// Pure decision: given an ADO build JSON body, what's the terminal
/// state from the operator's perspective?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildOutcome {
    /// `status` is anything but `completed`. Keep polling.
    InProgress,
    /// `status == "completed"` and `result == "succeeded"`.
    Succeeded,
    /// `status == "completed"` and `result` is anything else (failed,
    /// canceled, partiallySucceeded).
    Failed,
}

/// Pure function: classify a build's terminal state from its JSON
/// body. Tested independently of any HTTP code.
pub fn classify_build(body: &serde_json::Value) -> BuildOutcome {
    let status = body.get("status").and_then(|v| v.as_str()).unwrap_or("");
    if status != "completed" {
        return BuildOutcome::InProgress;
    }
    let result = body.get("result").and_then(|v| v.as_str()).unwrap_or("");
    if result == "succeeded" {
        BuildOutcome::Succeeded
    } else {
        BuildOutcome::Failed
    }
}

/// CLI options for [`dispatch`].
pub struct RunOptions<'a> {
    pub org: Option<&'a str>,
    pub project: Option<&'a str>,
    pub pat: Option<&'a str>,
    pub path: Option<&'a Path>,
    pub branch: Option<&'a str>,
    /// Raw `--parameters` arguments (one entry per CLI occurrence).
    pub parameters: &'a [String],
    pub wait: bool,
    pub poll_interval_secs: u64,
    pub timeout_secs: u64,
    pub dry_run: bool,
}

/// Run the `run` command — kept as `dispatch` to avoid the awkward
/// `run::run(...)` call site that a plain `run` would produce. See the
/// module-level comment.
pub async fn dispatch(opts: RunOptions<'_>) -> Result<()> {
    let parameters = parse_parameters(opts.parameters)?;

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
        .timeout(Duration::from_secs(30))
        .build()
        .context("Failed to create HTTP client")?;

    println!("Scanning for agentic workflows...");
    let detected = detect::detect_pipelines(&repo_path).await?;
    if detected.is_empty() {
        println!("No agentic workflows found.");
        return Ok(());
    }

    let matched = match_definitions(&client, &ado_ctx, &auth, &detected).await?;
    if matched.is_empty() {
        anyhow::bail!(
            "No ADO definitions matched any local fixture. Run `ado-aw list` to \
             diagnose."
        );
    }

    println!("{} definition(s) to queue.", matched.len());
    println!();

    if opts.dry_run {
        for m in &matched {
            print_queue_plan(m, opts.branch, &parameters);
        }
        return Ok(());
    }

    let mut targets: Vec<PollTarget> = Vec::new();
    let mut queue_failure = 0usize;

    for m in &matched {
        match queue_build(&client, &ado_ctx, &auth, m.id, opts.branch, &parameters).await {
            Ok(build_id) => {
                println!(
                    "▶ queued: {} (id={}) → build {} at {}/{}/_build/results?buildId={}",
                    m.name,
                    m.id,
                    build_id,
                    ado_ctx.org_url.trim_end_matches('/'),
                    percent_encoding::utf8_percent_encode(&ado_ctx.project, PATH_SEGMENT),
                    build_id
                );
                targets.push(PollTarget {
                    definition_id: m.id,
                    build_id,
                });
            }
            Err(e) => {
                eprintln!("✗ failed to queue: {} (id={}): {:#}", m.name, m.id, e);
                queue_failure += 1;
            }
        }
    }

    if !opts.wait {
        println!();
        println!(
            "Queued {} build(s); {} failed to queue.",
            targets.len(),
            queue_failure
        );
        if queue_failure > 0 {
            anyhow::bail!("{} build(s) failed to queue", queue_failure);
        }
        return Ok(());
    }

    // Deliberate design choice: when `--wait` is set and some builds
    // failed to queue, we still poll the successfully-queued ones
    // rather than bailing early. Three cases:
    //
    // - **Partial queue + at-least-one-queued**: `targets` is
    //   non-empty; the operator wants to know how those builds
    //   resolve. `queue_failure` is folded into the final exit code
    //   (non_success below).
    // - **Zero queued, queue_failure > 0**: `targets` is empty;
    //   `poll_until_complete` returns immediately with a default
    //   `PollOutcome`. We still print the wait summary so the
    //   operator sees a uniform report shape.
    // - **All queued**: the common path, no special handling needed.
    //
    // The early-exit path for `!opts.wait` above already bails on
    // queue_failure, so no further special-casing is required here.
    let poll_outcome = poll_until_complete(
        &client,
        &ado_ctx,
        &auth,
        &targets,
        Duration::from_secs(opts.poll_interval_secs),
        Duration::from_secs(opts.timeout_secs),
    )
    .await?;

    println!();
    println!(
        "Wait summary: {} succeeded, {} failed, {} still in progress (timeout), {} failed to queue.",
        poll_outcome.succeeded, poll_outcome.failed, poll_outcome.in_progress, queue_failure,
    );

    let non_success = poll_outcome.failed + poll_outcome.in_progress + queue_failure;
    if non_success > 0 {
        anyhow::bail!("not all builds succeeded");
    }
    Ok(())
}

fn print_queue_plan(
    m: &MatchedDefinition,
    branch: Option<&str>,
    parameters: &serde_json::Map<String, serde_json::Value>,
) {
    let mut body = serde_json::json!({
        "definition": { "id": m.id }
    });
    if let Some(b) = branch {
        body["sourceBranch"] = serde_json::Value::String(b.to_string());
    }
    if !parameters.is_empty() {
        body["templateParameters"] = serde_json::Value::Object(parameters.clone());
    }
    println!("[dry-run] ▶ would queue: {} (id={})", m.name, m.id);
    // The body is constructed in-line from primitive types and is
    // provably JSON-serializable, so `to_string_pretty` cannot fail
    // in practice. Surface any future regression as a visible token
    // rather than blank output (which would be invisible in the
    // dry-run feedback path).
    println!(
        "{}",
        serde_json::to_string_pretty(&body)
            .unwrap_or_else(|e| format!("<serialization error: {e}>"))
    );
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct PollOutcome {
    succeeded: usize,
    failed: usize,
    in_progress: usize,
}

/// Maximum consecutive poll errors per build before the poller gives
/// up on that specific target and counts it as failed. Bounds the
/// damage of a permanent error (deleted build, revoked PAT, 404)
/// without surrendering on a single transient blip.
const MAX_CONSECUTIVE_POLL_ERRORS: usize = 3;

async fn poll_until_complete(
    client: &reqwest::Client,
    ctx: &AdoContext,
    auth: &AdoAuth,
    targets: &[PollTarget],
    poll_interval: Duration,
    timeout: Duration,
) -> Result<PollOutcome> {
    let started = Instant::now();
    let mut outcome = PollOutcome::default();
    let mut pending: Vec<PollTarget> = targets.to_vec();
    // Consecutive poll-error count per build. A successful poll
    // (Succeeded / Failed / InProgress) resets the counter via the
    // implicit "we don't write to the map on success" — entries are
    // removed when the build leaves `pending`. The counter is
    // independent of `next_pending` so the bookkeeping stays
    // round-stable.
    let mut consecutive_errors: std::collections::HashMap<u64, usize> =
        std::collections::HashMap::new();

    println!();
    println!(
        "Waiting for {} build(s) (poll every {}s, timeout {}s)...",
        pending.len(),
        poll_interval.as_secs(),
        timeout.as_secs()
    );

    while !pending.is_empty() {
        if started.elapsed() >= timeout {
            println!("⚠ wait timed out after {}s", timeout.as_secs());
            outcome.in_progress = pending.len();
            return Ok(outcome);
        }

        let mut next_pending = Vec::new();
        let mut iter = pending.iter();
        let mut timed_out_mid_round = false;
        for t in iter.by_ref() {
            // Re-check the wall-clock budget between each in-flight
            // build, not just at the top of the round. With N targets
            // and a 30s reqwest timeout, the previous "check once per
            // round" loop could overshoot the operator's `--timeout`
            // by up to N × 30s in the pathological all-stalled case
            // — surprising behaviour when the poll interval is shorter
            // than the per-call HTTP timeout.
            if started.elapsed() >= timeout {
                // Carry the current target and every remaining one
                // forward so the caller's `in_progress` count is
                // accurate (the loop owes a status for everything it
                // queued).
                next_pending.push(*t);
                next_pending.extend(iter.by_ref().copied());
                timed_out_mid_round = true;
                break;
            }
            match get_build(client, ctx, auth, t.build_id).await {
                Ok(body) => {
                    consecutive_errors.remove(&t.build_id);
                    match classify_build(&body) {
                        BuildOutcome::InProgress => next_pending.push(*t),
                        BuildOutcome::Succeeded => {
                            println!("✓ build {} (definition {}) succeeded", t.build_id, t.definition_id);
                            outcome.succeeded += 1;
                        }
                        BuildOutcome::Failed => {
                            let result = body
                                .get("result")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown");
                            println!(
                                "✗ build {} (definition {}) finished with result={}",
                                t.build_id, t.definition_id, result
                            );
                            outcome.failed += 1;
                        }
                    }
                }
                Err(e) => {
                    let count = consecutive_errors.entry(t.build_id).or_insert(0);
                    *count += 1;
                    if *count >= MAX_CONSECUTIVE_POLL_ERRORS {
                        eprintln!(
                            "✗ build {} (definition {}): giving up after {} consecutive poll errors; last error: {:#}",
                            t.build_id, t.definition_id, count, e
                        );
                        // Count this as a failed build so the caller's
                        // exit code reflects the persistent error
                        // rather than waiting out --timeout.
                        outcome.failed += 1;
                        consecutive_errors.remove(&t.build_id);
                    } else {
                        eprintln!(
                            "  warning: poll error for build {} (definition {}) (attempt {}/{}): {:#}",
                            t.build_id,
                            t.definition_id,
                            count,
                            MAX_CONSECUTIVE_POLL_ERRORS,
                            e
                        );
                        // Treat as still-in-progress; we'll retry on
                        // the next tick.
                        next_pending.push(*t);
                    }
                }
            }
        }
        pending = next_pending;

        if timed_out_mid_round {
            println!("⚠ wait timed out after {}s", timeout.as_secs());
            outcome.in_progress = pending.len();
            return Ok(outcome);
        }

        if !pending.is_empty() {
            tokio::time::sleep(poll_interval).await;
        }
    }

    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============ parse_parameters ============

    #[test]
    fn parse_parameters_single_pair() {
        let m = parse_parameters(&["foo=bar".to_string()]).unwrap();
        assert_eq!(m.get("foo").unwrap().as_str(), Some("bar"));
    }

    #[test]
    fn parse_parameters_comma_separated() {
        let m = parse_parameters(&["foo=bar,baz=qux".to_string()]).unwrap();
        assert_eq!(m.get("foo").unwrap().as_str(), Some("bar"));
        assert_eq!(m.get("baz").unwrap().as_str(), Some("qux"));
    }

    #[test]
    fn parse_parameters_repeated() {
        let m = parse_parameters(&["a=1".to_string(), "b=2".to_string()]).unwrap();
        assert_eq!(m.get("a").unwrap().as_str(), Some("1"));
        assert_eq!(m.get("b").unwrap().as_str(), Some("2"));
    }

    #[test]
    fn parse_parameters_repeated_comma_mix() {
        let m =
            parse_parameters(&["a=1,b=2".to_string(), "c=3".to_string()]).unwrap();
        assert_eq!(m.len(), 3);
    }

    #[test]
    fn parse_parameters_value_with_equals() {
        // Split on first '=' only; subsequent equals are part of the value.
        let m = parse_parameters(&["key=a=b=c".to_string()]).unwrap();
        assert_eq!(m.get("key").unwrap().as_str(), Some("a=b=c"));
    }

    #[test]
    fn parse_parameters_rejects_missing_equals() {
        let err = parse_parameters(&["nope".to_string()]).unwrap_err();
        assert!(err.to_string().contains("no '='"), "got: {}", err);
    }

    #[test]
    fn parse_parameters_rejects_empty_key() {
        let err = parse_parameters(&["=bar".to_string()]).unwrap_err();
        assert!(err.to_string().contains("empty key"), "got: {}", err);
    }

    #[test]
    fn parse_parameters_empty_input_returns_empty() {
        let m = parse_parameters(&[]).unwrap();
        assert!(m.is_empty());
    }

    #[test]
    fn parse_parameters_skips_blank_pairs() {
        // Trailing/duplicate commas are forgiving.
        let m = parse_parameters(&["foo=bar,,".to_string()]).unwrap();
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn parse_parameters_values_with_commas_split_pre_equals() {
        // Documented sharp edge: each raw argument is split on `,` BEFORE
        // the `=` split. A value containing a comma will be torn apart
        // (and usually rejected because the trailing fragment has no `=`).
        // If you ever change parse_parameters to escape or quote commas,
        // update both the function doc and this test in lockstep — the
        // doc comment promises this exact behaviour.
        let err = parse_parameters(&["key=a,b".to_string()]).unwrap_err();
        assert!(
            err.to_string().contains("no '='"),
            "expected 'no =' error on the second fragment, got: {}",
            err
        );

        // The well-formed workaround is one --parameters flag per pair.
        let m = parse_parameters(&[
            "urls=https://a".to_string(),
            "extra=b".to_string(),
        ])
        .unwrap();
        assert_eq!(m.get("urls").unwrap().as_str(), Some("https://a"));
        assert_eq!(m.get("extra").unwrap().as_str(), Some("b"));
    }

    // ============ classify_build ============

    #[test]
    fn classify_in_progress_when_status_not_completed() {
        let body = serde_json::json!({ "status": "inProgress", "result": "succeeded" });
        assert_eq!(classify_build(&body), BuildOutcome::InProgress);
    }

    #[test]
    fn classify_in_progress_when_status_missing() {
        let body = serde_json::json!({ "result": "succeeded" });
        assert_eq!(classify_build(&body), BuildOutcome::InProgress);
    }

    #[test]
    fn classify_succeeded_when_completed_and_succeeded() {
        let body = serde_json::json!({ "status": "completed", "result": "succeeded" });
        assert_eq!(classify_build(&body), BuildOutcome::Succeeded);
    }

    #[test]
    fn classify_failed_when_completed_failed() {
        let body = serde_json::json!({ "status": "completed", "result": "failed" });
        assert_eq!(classify_build(&body), BuildOutcome::Failed);
    }

    #[test]
    fn classify_failed_when_completed_canceled() {
        let body = serde_json::json!({ "status": "completed", "result": "canceled" });
        assert_eq!(classify_build(&body), BuildOutcome::Failed);
    }

    #[test]
    fn classify_failed_when_completed_partial() {
        let body =
            serde_json::json!({ "status": "completed", "result": "partiallySucceeded" });
        assert_eq!(classify_build(&body), BuildOutcome::Failed);
    }

    #[test]
    fn classify_failed_when_completed_without_result() {
        let body = serde_json::json!({ "status": "completed" });
        assert_eq!(classify_build(&body), BuildOutcome::Failed);
    }
}
