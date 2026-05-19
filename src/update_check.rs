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
            if is_newer(latest, CURRENT_VERSION) {
                eprintln!(
                    "A newer version of ado-aw is available: v{latest} (you have v{CURRENT_VERSION}).\n\
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

/// Returns `true` when `latest` is strictly greater than `current`.
/// Both strings are expected to be bare semver triples, e.g. `"0.31.0"`.
/// Extra version components (pre-release suffixes, build metadata) are ignored.
fn is_newer(latest: &str, current: &str) -> bool {
    fn parse(s: &str) -> Option<(u64, u64, u64)> {
        let mut it = s.split('.');
        let major = it.next()?.parse().ok()?;
        let minor = it.next()?.parse().ok()?;
        // Allow patch to carry a pre-release suffix (e.g. "3-beta"); only the
        // leading numeric part matters for the comparison.
        let patch: u64 = it
            .next()?
            .split(|c: char| !c.is_ascii_digit())
            .next()
            .and_then(|n| n.parse().ok())?;
        Some((major, minor, patch))
    }

    match (parse(latest), parse(current)) {
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
