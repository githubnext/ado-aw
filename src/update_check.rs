//! Version update check.
//!
//! On every user-facing command invocation, queries the GitHub Releases API
//! for the latest `githubnext/ado-aw` release and prints an advisory message
//! to stderr when a newer version is available.  All network errors are
//! silently swallowed (logged at `debug` level) so a transient network hiccup
//! never interrupts the user's workflow.

use serde::Deserialize;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const RELEASES_API: &str =
    "https://api.github.com/repos/githubnext/ado-aw/releases/latest";

#[derive(Deserialize)]
struct LatestRelease {
    tag_name: String,
}

/// Check GitHub Releases for a newer version and, if one is found, print an
/// advisory to stderr.  Always returns `()` — errors are absorbed.
pub async fn check_for_update() {
    match fetch_latest_tag().await {
        Ok(tag) => {
            let latest = tag.trim_start_matches('v');
            // Only print if the version parses to a valid semver triple so we
            // never forward raw API content (e.g. ANSI escape sequences) to
            // the terminal.  Use the reconstructed string, not `latest`.
            if let Some((maj, min, pat)) = parse_version(latest)
                && (maj, min, pat) > parse_version(CURRENT_VERSION).unwrap_or((0, 0, 0))
            {
                eprintln!(
                    "A newer version of ado-aw is available: v{maj}.{min}.{pat} (you have v{CURRENT_VERSION}).\n\
                     Update at: https://github.com/githubnext/ado-aw/releases/latest"
                );
            }
        }
        Err(e) => {
            log::debug!("Update check failed (non-fatal): {e}");
        }
    }
}

async fn fetch_latest_tag() -> anyhow::Result<String> {
    let client = reqwest::Client::builder()
        .user_agent(format!("ado-aw/{CURRENT_VERSION}"))
        .timeout(std::time::Duration::from_secs(5))
        .build()?;

    let release: LatestRelease = client
        .get(RELEASES_API)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    Ok(release.tag_name)
}

/// Parse a bare semver string like `"0.31.0"` into `(major, minor, patch)`.
/// Pre-release suffixes on the patch component (e.g. `"3-beta"`) are accepted;
/// only the leading numeric part of patch is used.  Returns `None` if the
/// string is not a valid semver triple.
fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let mut it = s.split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next()?.parse().ok()?;
    let patch: u64 = it
        .next()?
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .and_then(|n| n.parse().ok())?;
    Some((major, minor, patch))
}

/// Returns `true` when `latest` is strictly greater than `current`.
/// Both strings are expected to be bare semver triples, e.g. `"0.31.0"`.
/// Extra version components (pre-release suffixes, build metadata) are ignored.
#[cfg(test)]
fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::is_newer;

    #[test]
    fn newer_patch() {
        assert!(is_newer("0.30.3", "0.30.2"));
    }

    #[test]
    fn newer_minor() {
        assert!(is_newer("0.31.0", "0.30.2"));
    }

    #[test]
    fn newer_major() {
        assert!(is_newer("1.0.0", "0.30.2"));
    }

    #[test]
    fn same_version() {
        assert!(!is_newer("0.30.2", "0.30.2"));
    }

    #[test]
    fn older_version() {
        assert!(!is_newer("0.29.0", "0.30.2"));
    }

    #[test]
    fn v_prefix_already_stripped_by_caller() {
        // check_for_update() strips the 'v' before calling is_newer; verify
        // that stripping works end-to-end by simulating it here.
        let tag = "v0.31.0";
        let stripped = tag.trim_start_matches('v');
        assert!(is_newer(stripped, "0.30.2"));
    }
}
