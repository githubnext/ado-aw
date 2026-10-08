//! `runtimes.<x>.feed-url` pointing at Azure Artifacts ->
//! `permissions.packages.feeds` + `runtimes.<x>.feed`.
//!
//! Azure Artifacts feeds always require a credential. The authenticate tasks
//! that used to supply one leaked it into the agent sandbox and were removed,
//! so an Artifacts `feed-url` no longer works on its own. The supported shape
//! grants the feed under `permissions.packages` (credential held by the
//! `ado-proxy`) and selects it with `runtimes.<x>.feed`.
//!
//! The migration preserves the old behaviour: the job's build identity is
//! still the credential, and because that identity could save packages from
//! upstream sources before, the migrated grant opts into `upstream: allow`
//! unless the URL already named a feed view.

use anyhow::{Result, bail};
use serde_yaml::{Mapping, Sequence, Value};

use super::{Codemod, CodemodContext};

const INTRODUCED_IN: &str = "0.54.0";

pub static CODEMOD: Codemod = Codemod {
    id: "package_feed_permissions",
    summary: "Azure Artifacts runtimes.<x>.feed-url moved to permissions.packages and runtimes.<x>.feed",
    introduced_in: INTRODUCED_IN,
    apply: apply_codemod,
};

/// Runtime key and the package protocol its package manager speaks.
const RUNTIMES: &[(&str, &str)] = &[("python", "pypi"), ("node", "npm"), ("dotnet", "nuget")];

fn key(name: &str) -> Value {
    Value::String(name.to_string())
}

/// Decode `%XX` escapes. Returns `None` for malformed escapes or non-UTF-8.
fn percent_decode(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = segment.get(index + 1..index + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// A feed parsed from an Azure Artifacts package-source URL.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedFeed {
    organization: String,
    project: Option<String>,
    feed: String,
    view: Option<String>,
}

impl ParsedFeed {
    fn identity(&self) -> String {
        format!(
            "{}/{}/{}@{}",
            self.organization.to_ascii_lowercase(),
            self.project.as_deref().unwrap_or("").to_ascii_lowercase(),
            self.feed.to_ascii_lowercase(),
            self.view.as_deref().unwrap_or("").to_ascii_lowercase()
        )
    }
}

/// Parse an Azure Artifacts package-source URL.
///
/// Returns `Ok(None)` for URLs on other hosts (left for the author), and an
/// error for an Artifacts URL whose shape cannot be migrated safely.
fn parse_artifacts_url(url: &str) -> Result<Option<ParsedFeed>> {
    let Some(rest) = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")) else {
        return Ok(None);
    };
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.to_ascii_lowercase();
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let mut segments: Vec<&str> = path.split('/').filter(|segment| !segment.is_empty()).collect();

    let organization = if host == "pkgs.dev.azure.com" {
        if segments.is_empty() {
            bail!("cannot migrate feed-url '{url}': it names no organization");
        }
        segments.remove(0).to_string()
    } else if let Some(organization) = host.strip_suffix(".pkgs.visualstudio.com") {
        organization.to_string()
    } else {
        return Ok(None);
    };

    let Some(packaging) = segments
        .iter()
        .position(|segment| segment.eq_ignore_ascii_case("_packaging"))
    else {
        bail!("cannot migrate feed-url '{url}': it has no `_packaging/<feed>` segment");
    };
    let project = match packaging {
        0 => None,
        1 => Some(segments[0]),
        _ => bail!("cannot migrate feed-url '{url}': unexpected path before `_packaging`"),
    };
    let Some(feed_segment) = segments.get(packaging + 1) else {
        bail!("cannot migrate feed-url '{url}': it names no feed after `_packaging`");
    };
    let decoded = percent_decode(feed_segment)
        .ok_or_else(|| anyhow::anyhow!("cannot migrate feed-url '{url}': invalid encoding"))?;
    let (feed, view) = match decoded.split_once('@') {
        Some((feed, view)) => (feed.to_string(), Some(view.to_string())),
        None => (decoded, None),
    };
    let project = match project {
        Some(project) => Some(percent_decode(project).ok_or_else(|| {
            anyhow::anyhow!("cannot migrate feed-url '{url}': invalid project encoding")
        })?),
        None => None,
    };
    let organization = percent_decode(&organization)
        .ok_or_else(|| anyhow::anyhow!("cannot migrate feed-url '{url}': invalid encoding"))?;

    // Validate with the same newtypes the typed front matter uses, so a
    // migrated source can never fail to deserialize.
    crate::secure::AdoOrganization::parse(organization.as_str())?;
    crate::secure::AdoFeedName::parse(feed.as_str())?;
    if let Some(view) = &view {
        crate::secure::AdoFeedName::parse(view.as_str())?;
    }
    if let Some(project) = &project {
        crate::secure::AdoProject::parse(project.as_str())?;
    }
    Ok(Some(ParsedFeed {
        organization,
        project,
        feed,
        view,
    }))
}

fn feeds_sequence(fm: &mut Mapping) -> Result<&mut Sequence> {
    let permissions = fm
        .entry(key("permissions"))
        .or_insert_with(|| Value::Mapping(Mapping::new()));
    let Some(permissions) = permissions.as_mapping_mut() else {
        bail!("cannot migrate runtime feed-url: `permissions` is not a mapping");
    };
    let packages = permissions
        .entry(key("packages"))
        .or_insert_with(|| Value::Mapping(Mapping::new()));
    let Some(packages) = packages.as_mapping_mut() else {
        bail!("cannot migrate runtime feed-url: `permissions.packages` is not a mapping");
    };
    let feeds = packages
        .entry(key("feeds"))
        .or_insert_with(|| Value::Sequence(Sequence::new()));
    feeds.as_sequence_mut().ok_or_else(|| {
        anyhow::anyhow!("cannot migrate runtime feed-url: `permissions.packages.feeds` is not a list")
    })
}

fn entry_identity(entry: &Mapping) -> Option<String> {
    let text = |name: &str| entry.get(key(name)).and_then(Value::as_str).unwrap_or("");
    Some(format!(
        "{}/{}/{}@{}",
        text("organization").to_ascii_lowercase(),
        text("project").to_ascii_lowercase(),
        entry.get(key("feed"))?.as_str()?.to_ascii_lowercase(),
        text("view").to_ascii_lowercase()
    ))
}

fn entry_handle(entry: &Mapping) -> Option<String> {
    entry
        .get(key("name"))
        .or_else(|| entry.get(key("feed")))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Find or add the grant for `parsed`, returning its handle.
fn upsert_grant(feeds: &mut Sequence, parsed: &ParsedFeed, protocol: &str) -> Result<String> {
    let identity = parsed.identity();
    for entry in feeds.iter_mut() {
        let Some(entry) = entry.as_mapping_mut() else {
            continue;
        };
        if entry_identity(entry).as_deref() != Some(identity.as_str()) {
            continue;
        }
        let protocols = entry
            .entry(key("protocols"))
            .or_insert_with(|| Value::Sequence(Sequence::new()));
        let Some(protocols) = protocols.as_sequence_mut() else {
            bail!("cannot migrate runtime feed-url: a feed's `protocols` is not a list");
        };
        if !protocols.iter().any(|value| value.as_str() == Some(protocol)) {
            protocols.push(Value::String(protocol.to_string()));
        }
        return entry_handle(entry)
            .ok_or_else(|| anyhow::anyhow!("cannot migrate runtime feed-url: feed has no name"));
    }

    let taken: Vec<String> = feeds
        .iter()
        .filter_map(Value::as_mapping)
        .filter_map(entry_handle)
        .map(|handle| handle.to_ascii_lowercase())
        .collect();
    let base = match &parsed.view {
        Some(view) => format!("{}-{}", parsed.feed, view),
        None => parsed.feed.clone(),
    };
    let mut handle = base.clone();
    let mut suffix = 2;
    while taken.contains(&handle.to_ascii_lowercase()) {
        handle = format!("{base}-{suffix}");
        suffix += 1;
    }
    crate::secure::AdoFeedName::parse(handle.as_str())?;

    let mut entry = Mapping::new();
    entry.insert(key("name"), Value::String(handle.clone()));
    entry.insert(key("organization"), Value::String(parsed.organization.clone()));
    if let Some(project) = &parsed.project {
        entry.insert(key("project"), Value::String(project.clone()));
    }
    entry.insert(key("feed"), Value::String(parsed.feed.clone()));
    if let Some(view) = &parsed.view {
        entry.insert(key("view"), Value::String(view.clone()));
    } else {
        // Before this migration the agent read the feed with the build
        // identity, which can save upstream packages. Keep that behaviour
        // explicit; narrow it with `view:` or `identity-role: reader`.
        entry.insert(key("upstream"), Value::String("allow".to_string()));
    }
    entry.insert(
        key("protocols"),
        Value::Sequence(vec![Value::String(protocol.to_string())]),
    );
    feeds.push(Value::Mapping(entry));
    Ok(handle)
}

fn apply_codemod(fm: &mut Mapping, _ctx: &CodemodContext) -> Result<bool> {
    // Collect first: the runtimes and permissions subtrees are both mutated.
    let mut pending: Vec<(&'static str, &'static str, ParsedFeed)> = Vec::new();
    if let Some(runtimes) = fm.get(key("runtimes")).and_then(Value::as_mapping) {
        for (runtime, protocol) in RUNTIMES {
            let Some(config) = runtimes.get(key(runtime)).and_then(Value::as_mapping) else {
                continue;
            };
            let Some(url) = config.get(key("feed-url")).and_then(Value::as_str) else {
                continue;
            };
            let Some(parsed) = parse_artifacts_url(url)? else {
                continue;
            };
            if config.contains_key(key("feed")) {
                bail!(
                    "runtimes.{runtime} sets both `feed-url` and `feed`; manual migration \
                     required: remove `feed-url` and grant the feed under `permissions.packages`"
                );
            }
            pending.push((runtime, protocol, parsed));
        }
    }
    if pending.is_empty() {
        return Ok(false);
    }

    let mut handles = Vec::with_capacity(pending.len());
    {
        let feeds = feeds_sequence(fm)?;
        for (runtime, protocol, parsed) in &pending {
            handles.push((*runtime, upsert_grant(feeds, parsed, protocol)?));
        }
    }

    let runtimes = fm
        .get_mut(key("runtimes"))
        .and_then(Value::as_mapping_mut)
        .expect("runtimes was read above");
    for (runtime, handle) in handles {
        let config = runtimes
            .get_mut(key(runtime))
            .and_then(Value::as_mapping_mut)
            .expect("runtime was read above");
        config.remove(key("feed-url"));
        config.insert(key("feed"), Value::String(handle));
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> CodemodContext {
        CodemodContext {
            compiler_version: INTRODUCED_IN,
            source_compiler_version: None,
        }
    }

    fn map(yaml: &str) -> Mapping {
        serde_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn migrates_each_runtime_and_merges_a_shared_feed() {
        let mut fm = map(
            "runtimes:\n  python:\n    version: '3.12'\n    feed-url: https://pkgs.dev.azure.com/contoso/_packaging/internal/pypi/simple/\n  \
             node:\n    feed-url: https://pkgs.dev.azure.com/contoso/_packaging/internal/npm/registry/\n  \
             dotnet:\n    feed-url: https://pkgs.dev.azure.com/contoso/My%20Project/_packaging/rel@Release/nuget/v3/index.json\n",
        );
        assert!(apply_codemod(&mut fm, &ctx()).unwrap());

        assert_eq!(fm["runtimes"]["python"]["feed"], "internal");
        assert_eq!(fm["runtimes"]["python"]["version"], "3.12");
        assert!(fm["runtimes"]["python"].get("feed-url").is_none());
        assert_eq!(fm["runtimes"]["node"]["feed"], "internal");
        assert_eq!(fm["runtimes"]["dotnet"]["feed"], "rel-Release");

        let feeds = fm["permissions"]["packages"]["feeds"].as_sequence().unwrap();
        assert_eq!(feeds.len(), 2);
        assert_eq!(feeds[0]["organization"], "contoso");
        assert_eq!(feeds[0]["upstream"], "allow");
        assert_eq!(
            feeds[0]["protocols"],
            Value::Sequence(vec![key("pypi"), key("npm")])
        );
        assert_eq!(feeds[1]["project"], "My Project");
        assert_eq!(feeds[1]["view"], "Release");
        assert!(feeds[1].get("upstream").is_none());

        // The migrated front matter must type-check and validate.
        let typed: crate::compile::types::PermissionsConfig =
            serde_yaml::from_value(fm["permissions"].clone()).unwrap();
        typed.packages.unwrap().validate().unwrap();
    }

    #[test]
    fn legacy_hosts_and_existing_grants_are_respected() {
        let mut fm = map(
            "permissions:\n  packages:\n    service-connection: wif\n    feeds:\n      - name: internal\n        \
             feed: other\n        upstream: allow\n        protocols: [cargo]\n\
             runtimes:\n  node:\n    feed-url: https://contoso.pkgs.visualstudio.com/_packaging/internal/npm/registry/\n",
        );
        assert!(apply_codemod(&mut fm, &ctx()).unwrap());
        assert_eq!(fm["permissions"]["packages"]["service-connection"], "wif");
        assert_eq!(fm["runtimes"]["node"]["feed"], "internal-2");
        let feeds = fm["permissions"]["packages"]["feeds"].as_sequence().unwrap();
        assert_eq!(feeds[1]["organization"], "contoso");
    }

    #[test]
    fn other_hosts_and_absent_runtimes_are_noops_and_migration_is_idempotent() {
        let mut fm = map(
            "runtimes:\n  node:\n    feed-url: https://registry.example.test/npm/\n  python: true\n",
        );
        let snapshot = fm.clone();
        assert!(!apply_codemod(&mut fm, &ctx()).unwrap());
        assert_eq!(fm, snapshot);

        let mut fm = map(
            "runtimes:\n  node:\n    feed-url: https://pkgs.dev.azure.com/o/_packaging/f/npm/registry/\n",
        );
        assert!(apply_codemod(&mut fm, &ctx()).unwrap());
        let snapshot = fm.clone();
        assert!(!apply_codemod(&mut fm, &ctx()).unwrap());
        assert_eq!(fm, snapshot);
    }

    #[test]
    fn unmigratable_or_conflicting_shapes_fail_loudly() {
        for yaml in [
            "runtimes:\n  node:\n    feed-url: https://pkgs.dev.azure.com/o/_packaging/f/npm/registry/\n    feed: x\n",
            "runtimes:\n  node:\n    feed-url: https://pkgs.dev.azure.com/o/npm/registry/\n",
            "runtimes:\n  node:\n    feed-url: https://pkgs.dev.azure.com/o/a/b/_packaging/f/npm/registry/\n",
            "permissions: read-only\nruntimes:\n  node:\n    feed-url: https://pkgs.dev.azure.com/o/_packaging/f/npm/registry/\n",
        ] {
            assert!(apply_codemod(&mut map(yaml), &ctx()).is_err(), "{yaml}");
        }
    }
}
