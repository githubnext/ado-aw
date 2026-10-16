//! Warning-only check for Azure Artifacts URLs on the legacy
//! `{org}.pkgs.visualstudio.com` host in committed package-manager config.
//!
//! The package proxy (`permissions.packages`) intercepts only the canonical
//! `pkgs.dev.azure.com` host. A repository config file that still names the
//! legacy host sends the agent's restores straight to Azure Artifacts with no
//! credential, so they fail with `401` and nothing explains why. Front matter
//! is already migrated by codemod 0009; this check covers the files the
//! compiler does not own.
//!
//! Only the repository root (the git root of the workflow source) is scanned,
//! for a fixed list of well-known file names. That keeps the check cheap and
//! predictable; a config file in a subdirectory is not seen.

use std::path::Path;

use regex_lite::Regex;

use crate::compile::types::FrontMatter;

/// Package-manager config files checked at the repository root. Single-segment
/// names match case-insensitively (`NuGet.Config`, `nuget.config`, ...).
const CONFIG_FILES: &[&str] = &[
    "nuget.config",
    ".npmrc",
    ".yarnrc.yml",
    "pip.conf",
    "pip.ini",
    "uv.toml",
    "pyproject.toml",
    ".cargo/config.toml",
    ".cargo/config",
];

/// Files larger than this are skipped; package config is small.
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

/// One legacy-host URL found in a committed config file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyFeedUrl {
    /// Repository-relative path, with `/` separators.
    pub file: String,
    /// 1-based line of the first occurrence of this URL in the file.
    pub line: usize,
    /// Organization named by the legacy host.
    pub organization: String,
    /// The URL as written (scheme optional, e.g. npm's `//host/...` keys).
    pub url: String,
    /// The equivalent URL on the canonical host.
    pub canonical: String,
}

impl LegacyFeedUrl {
    /// Author-facing explanation, shared by `compile` and `ado-aw lint`.
    pub fn message(&self, packages_configured: bool) -> String {
        let grant = if packages_configured {
            ""
        } else {
            ", and grant the feed under `permissions.packages`"
        };
        format!(
            "`{}:{}` uses the legacy Azure Artifacts host `{}.pkgs.visualstudio.com`. \
             The package proxy only authenticates `pkgs.dev.azure.com`, so restores from \
             this URL reach the feed without a credential and fail with 401. Change it to \
             `{}`{grant}.",
            self.file, self.line, self.organization, self.canonical
        )
    }
}

/// Whether the workflow can restore packages, so the check is relevant.
pub fn applies_to(front_matter: &FrontMatter) -> bool {
    let packages = front_matter
        .permissions
        .as_ref()
        .is_some_and(|permissions| permissions.packages.is_some());
    let runtime = front_matter.runtimes.as_ref().is_some_and(|runtimes| {
        runtimes.python.as_ref().is_some_and(|c| c.is_enabled())
            || runtimes.node.as_ref().is_some_and(|c| c.is_enabled())
            || runtimes.dotnet.as_ref().is_some_and(|c| c.is_enabled())
    });
    packages || runtime
}

fn legacy_url_pattern() -> Regex {
    // Scheme optional: `.npmrc` keys use the protocol-relative `//host/...`.
    // The path stops at `:` so an npm `/:_authToken` suffix is not included.
    Regex::new(r#"(?i)(https?:)?//([a-z0-9][a-z0-9-]{0,62})\.pkgs\.visualstudio\.com(/[^\s"'<>:]*)?"#)
        .expect("static regex is valid")
}

/// Legacy-host URLs in `content`, one per distinct URL, in file order.
fn scan_content(file: &str, content: &str, pattern: &Regex) -> Vec<LegacyFeedUrl> {
    let mut found: Vec<LegacyFeedUrl> = Vec::new();
    for (index, line) in content.lines().enumerate() {
        for captures in pattern.captures_iter(line) {
            let url = captures[0].to_string();
            if found.iter().any(|entry| entry.url.eq_ignore_ascii_case(&url)) {
                continue;
            }
            let organization = captures[2].to_ascii_lowercase();
            let path = captures.get(3).map_or("/", |m| m.as_str());
            let prefix = if captures.get(1).is_some() { "https:" } else { "" };
            found.push(LegacyFeedUrl {
                file: file.to_string(),
                line: index + 1,
                canonical: format!("{prefix}//pkgs.dev.azure.com/{organization}{path}"),
                organization,
                url,
            });
        }
    }
    found
}

/// Read the well-known package config files under `root`, as
/// `(repo-relative path, content)` pairs in [`CONFIG_FILES`] order.
fn read_configs(root: &Path) -> Vec<(String, String)> {
    let root_entries: Vec<String> = std::fs::read_dir(root)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
                .filter_map(|entry| entry.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();

    let mut configs = Vec::new();
    for wanted in CONFIG_FILES {
        let relative = if wanted.contains('/') {
            Some((*wanted).to_string())
        } else {
            root_entries
                .iter()
                .find(|name| name.eq_ignore_ascii_case(wanted))
                .cloned()
        };
        let Some(relative) = relative else {
            continue;
        };
        let path = root.join(&relative);
        let small_file = std::fs::metadata(&path)
            .is_ok_and(|meta| meta.is_file() && meta.len() <= MAX_CONFIG_BYTES);
        if !small_file {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(&path) {
            configs.push((relative, content));
        }
    }
    configs
}

/// Scan the well-known package config files under `root` for legacy-host URLs.
pub fn scan_repository(root: &Path) -> Vec<LegacyFeedUrl> {
    let pattern = legacy_url_pattern();
    read_configs(root)
        .iter()
        .flat_map(|(file, content)| scan_content(file, content, &pattern))
        .collect()
}

/// Why the proxy would refuse a canonical feed URL found in config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UngrantedReason {
    /// `permissions.packages` is absent, so no credential is ever attached.
    NoPackagesPermission,
    /// No grant matches the organization, project, and feed.
    FeedNotGranted,
    /// The feed is granted only through this view.
    ViewNotGranted { view: String },
    /// The feed is granted, but not for this protocol.
    ProtocolNotGranted { protocol: &'static str },
    /// The path under the feed is not a protocol the proxy serves.
    UnsupportedRoute,
}

/// A canonical Azure Artifacts URL in committed config that the package proxy
/// would refuse (or never authenticate).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UngrantedFeedUrl {
    /// Repository-relative path, with `/` separators.
    pub file: String,
    /// 1-based line of the first occurrence of this URL in the file.
    pub line: usize,
    /// The URL as written.
    pub url: String,
    /// `organization[/project]/feed[@view]` as named by the URL.
    pub feed: String,
    pub reason: UngrantedReason,
}

impl UngrantedFeedUrl {
    /// Author-facing explanation, shared by `compile` and `ado-aw lint`.
    pub fn message(&self) -> String {
        let at = format!("`{}:{}` points at Azure Artifacts feed `{}`", self.file, self.line, self.feed);
        match &self.reason {
            UngrantedReason::NoPackagesPermission => format!(
                "{at}, but `permissions.packages` is not configured, so restores from it carry \
                 no credential and fail with 401. Grant the feed under `permissions.packages`."
            ),
            UngrantedReason::FeedNotGranted => format!(
                "{at}, which `permissions.packages` does not grant, so the package proxy refuses \
                 it (403 `feed-not-granted`). Add a grant whose organization, project, and feed \
                 match the URL; an organization-scoped grant does not cover a project-scoped URL."
            ),
            UngrantedReason::ViewNotGranted { view } => format!(
                "{at}, but its grant is pinned to view `{view}`, so the package proxy refuses \
                 this URL (403 `view-not-granted`). Address the feed as `{{feed}}@{view}`."
            ),
            UngrantedReason::ProtocolNotGranted { protocol } => format!(
                "{at} over the `{protocol}` protocol, which its grant does not list, so the \
                 package proxy refuses it (403 `protocol-not-granted`). Add `{protocol}` to the \
                 grant's `protocols`."
            ),
            UngrantedReason::UnsupportedRoute => format!(
                "{at}, but the path under the feed is not a protocol the package proxy serves \
                 (npm, PyPI, NuGet v3, Cargo), so it is refused (403 `unknown-route`)."
            ),
        }
    }
}

fn canonical_url_pattern() -> Regex {
    // Same shape as the legacy pattern: scheme optional, path stops at `:`.
    Regex::new(r#"(?i)(?:https?:)?//pkgs\.dev\.azure\.com(/[^\s"'<>:]*)"#)
        .expect("static regex is valid")
}

/// Decode `%XX` escapes in one path segment. `None` for malformed escapes.
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

/// A feed route parsed from a canonical URL path, mirroring the proxy's
/// `matchFeedRoute`: `/{org}[/{project}]/_packaging/{feed}[@{view}]/{rest}`.
struct FeedRoute {
    organization: String,
    project: Option<String>,
    feed: String,
    view: Option<String>,
    rest: Vec<String>,
}

fn parse_feed_route(path: &str) -> Option<FeedRoute> {
    let segments: Vec<String> = path
        .split('/')
        .skip(1)
        .map(percent_decode)
        .collect::<Option<_>>()?;
    let packaging = (1..=2).find(|&index| {
        segments
            .get(index)
            .is_some_and(|segment| segment.eq_ignore_ascii_case("_packaging"))
    })?;
    let scope = &segments[..packaging];
    let feed_segment = segments.get(packaging + 1)?;
    if scope.iter().any(String::is_empty) || feed_segment.is_empty() {
        return None;
    }
    let (feed, view) = match feed_segment.split_once('@') {
        Some((feed, view)) if !feed.is_empty() && !view.is_empty() && !view.contains('@') => {
            (feed.to_string(), Some(view.to_string()))
        }
        Some(_) => return None,
        None => (feed_segment.clone(), None),
    };
    Some(FeedRoute {
        organization: scope[0].clone(),
        project: scope.get(1).cloned(),
        feed,
        view,
        rest: segments[packaging + 2..].to_vec(),
    })
}

/// The protocol whose catalogued prefix the configured base URL starts with.
///
/// A config URL is a *base* the client appends to, so the prefix may be the
/// whole remainder (`…/npm/registry/` or `…/npm/registry`).
fn route_protocol(rest: &[String]) -> Option<crate::ado_proxy::catalog::PackageProtocolId> {
    let rest: Vec<&str> = rest.iter().map(String::as_str).filter(|s| !s.is_empty()).collect();
    crate::ado_proxy::catalog::package_protocols()
        .into_iter()
        .find(|route| {
            route.path_prefixes.iter().any(|prefix| {
                let parts: Vec<&str> = prefix.split('/').filter(|part| !part.is_empty()).collect();
                rest.len() >= parts.len()
                    && parts
                        .iter()
                        .zip(&rest)
                        .all(|(part, segment)| part.eq_ignore_ascii_case(segment))
            })
        })
        .map(|route| route.protocol)
}

/// Whether the proxy would refuse `route`, or `None` when it would allow it
/// or the outcome cannot be decided statically.
///
/// Mirrors `authorizePackageRequest` in the proxy. A GUID in the URL can only
/// be compared after `resolve-feeds` runs, and a grant without `organization`
/// takes the pipeline's organization at runtime; both cases are treated as
/// matching, so the check never reports a URL the proxy would allow.
fn refusal(
    route: &FeedRoute,
    packages: Option<&crate::compile::types::PackagesPermissionConfig>,
) -> Option<UngrantedReason> {
    use crate::validate::is_valid_guid;
    if [&route.organization, &route.feed]
        .into_iter()
        .chain(route.project.as_ref())
        .chain(route.view.as_ref())
        .any(|value| is_valid_guid(value))
    {
        return None;
    }
    let Some(packages) = packages else {
        return Some(UngrantedReason::NoPackagesPermission);
    };
    let Some(protocol) = route_protocol(&route.rest) else {
        return Some(UngrantedReason::UnsupportedRoute);
    };

    let feed_grants: Vec<_> = packages
        .feeds
        .iter()
        .filter(|grant| {
            let organization = grant
                .organization
                .as_deref()
                .is_none_or(|organization| organization.eq_ignore_ascii_case(&route.organization));
            let project = match (grant.project.as_deref(), route.project.as_deref()) {
                (None, None) => true,
                (Some(granted), Some(requested)) => granted.eq_ignore_ascii_case(requested),
                _ => false,
            };
            organization && project && grant.feed.eq_ignore_ascii_case(&route.feed)
        })
        .collect();
    if feed_grants.is_empty() {
        return Some(UngrantedReason::FeedNotGranted);
    }
    let view_grants: Vec<_> = feed_grants
        .iter()
        .filter(|grant| match grant.view.as_deref() {
            None => true,
            Some(view) => route
                .view
                .as_deref()
                .is_some_and(|requested| view.eq_ignore_ascii_case(requested)),
        })
        .collect();
    if view_grants.is_empty() {
        let view = feed_grants[0].view.as_deref().unwrap_or_default().to_string();
        return Some(UngrantedReason::ViewNotGranted { view });
    }
    if !view_grants
        .iter()
        .any(|grant| grant.protocols.iter().any(|p| p.to_catalog() == protocol))
    {
        return Some(UngrantedReason::ProtocolNotGranted {
            protocol: protocol_name(protocol),
        });
    }
    None
}

/// Front-matter spelling of a catalog protocol.
fn protocol_name(protocol: crate::ado_proxy::catalog::PackageProtocolId) -> &'static str {
    use crate::ado_proxy::catalog::PackageProtocolId;
    match protocol {
        PackageProtocolId::Npm => "npm",
        PackageProtocolId::Pypi => "pypi",
        PackageProtocolId::Nuget => "nuget",
        PackageProtocolId::Cargo => "cargo",
    }
}

fn scan_content_grants(
    file: &str,
    content: &str,
    pattern: &Regex,
    packages: Option<&crate::compile::types::PackagesPermissionConfig>,
) -> Vec<UngrantedFeedUrl> {
    let mut seen: Vec<String> = Vec::new();
    let mut found = Vec::new();
    for (index, line) in content.lines().enumerate() {
        for captures in pattern.captures_iter(line) {
            let url = captures[0].to_string();
            if seen.iter().any(|known| known.eq_ignore_ascii_case(&url)) {
                continue;
            }
            seen.push(url.clone());
            let Some(route) = parse_feed_route(&captures[1]) else {
                continue;
            };
            let Some(reason) = refusal(&route, packages) else {
                continue;
            };
            let mut feed = route.organization.clone();
            if let Some(project) = &route.project {
                feed.push('/');
                feed.push_str(project);
            }
            feed.push('/');
            feed.push_str(&route.feed);
            if let Some(view) = &route.view {
                feed.push('@');
                feed.push_str(view);
            }
            found.push(UngrantedFeedUrl {
                file: file.to_string(),
                line: index + 1,
                url,
                feed,
                reason,
            });
        }
    }
    found
}

/// Scan the well-known package config files under `root` for canonical
/// Azure Artifacts URLs that `packages` does not cover.
pub fn scan_repository_grants(
    root: &Path,
    packages: Option<&crate::compile::types::PackagesPermissionConfig>,
) -> Vec<UngrantedFeedUrl> {
    let pattern = canonical_url_pattern();
    read_configs(root)
        .iter()
        .flat_map(|(file, content)| scan_content_grants(file, content, &pattern, packages))
        .collect()
}

/// Every finding for one repository.
#[derive(Debug, Default)]
pub struct ConfigFindings {
    pub legacy: Vec<LegacyFeedUrl>,
    pub ungranted: Vec<UngrantedFeedUrl>,
}

/// Findings for the repository containing `source_path`, or nothing when the
/// workflow cannot restore packages or the source is not in a git repository.
pub fn check_source(front_matter: &FrontMatter, source_path: &Path) -> ConfigFindings {
    if !applies_to(front_matter) {
        return ConfigFindings::default();
    }
    let absolute = std::path::absolute(source_path).unwrap_or_else(|_| source_path.to_path_buf());
    let Some(root) = super::common::find_git_root(&absolute) else {
        return ConfigFindings::default();
    };
    let packages = front_matter
        .permissions
        .as_ref()
        .and_then(|permissions| permissions.packages.as_ref());
    ConfigFindings {
        legacy: scan_repository(&root),
        ungranted: scan_repository_grants(&root, packages),
    }
}

/// Compile-time warnings for the repository containing `source_path`.
pub fn collect_warnings(front_matter: &FrontMatter, source_path: &Path) -> Vec<String> {
    let packages_configured = front_matter
        .permissions
        .as_ref()
        .is_some_and(|permissions| permissions.packages.is_some());
    let findings = check_source(front_matter, source_path);
    findings
        .legacy
        .iter()
        .map(|finding| finding.message(packages_configured))
        .chain(findings.ungranted.iter().map(UngrantedFeedUrl::message))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn front_matter(yaml: &str) -> FrontMatter {
        crate::compile::parse_markdown(&format!("---\nname: t\ndescription: x\n{yaml}---\n"))
            .unwrap()
            .0
    }

    fn repo(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        for (name, content) in files {
            let path = dir.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
        dir
    }

    #[test]
    fn finds_legacy_urls_with_lines_and_canonical_replacements() {
        let dir = repo(&[
            (
                "NuGet.Config",
                "<configuration>\n  <packageSources>\n    \
                 <add key=\"internal\" value=\"https://Contoso.pkgs.visualstudio.com/_packaging/internal/nuget/v3/index.json\" />\n",
            ),
            (
                ".npmrc",
                "registry=https://contoso.pkgs.visualstudio.com/Eng/_packaging/web/npm/registry/\n\
                 always-auth=true\n\
                 //contoso.pkgs.visualstudio.com/Eng/_packaging/web/npm/registry/:_authToken=${TOKEN}\n",
            ),
            (".cargo/config.toml", "[registries.internal]\nindex = \"sparse+https://contoso.pkgs.visualstudio.com/_packaging/crates/Cargo/index/\"\n"),
        ]);
        let findings = scan_repository(dir.path());
        let summary: Vec<(String, usize, String)> = findings
            .iter()
            .map(|f| (f.file.clone(), f.line, f.canonical.clone()))
            .collect();
        assert_eq!(
            summary,
            vec![
                (
                    "NuGet.Config".to_string(),
                    3,
                    "https://pkgs.dev.azure.com/contoso/_packaging/internal/nuget/v3/index.json".to_string()
                ),
                (
                    ".npmrc".to_string(),
                    1,
                    "https://pkgs.dev.azure.com/contoso/Eng/_packaging/web/npm/registry/".to_string()
                ),
                (
                    ".npmrc".to_string(),
                    3,
                    "//pkgs.dev.azure.com/contoso/Eng/_packaging/web/npm/registry/".to_string()
                ),
                (
                    ".cargo/config.toml".to_string(),
                    2,
                    "https://pkgs.dev.azure.com/contoso/_packaging/crates/Cargo/index/".to_string()
                ),
            ]
        );
        assert_eq!(findings[0].organization, "contoso");
    }

    #[test]
    fn canonical_urls_other_hosts_and_unlisted_files_are_ignored() {
        let dir = repo(&[
            (".npmrc", "registry=https://pkgs.dev.azure.com/contoso/_packaging/web/npm/registry/\n"),
            ("pip.conf", "[global]\nindex-url = https://pypi.org/simple/\n"),
            ("src/nuget.config", "https://contoso.pkgs.visualstudio.com/_packaging/x/nuget/v3/index.json"),
            ("README.md", "https://contoso.pkgs.visualstudio.com/_packaging/x/npm/registry/"),
        ]);
        assert!(scan_repository(dir.path()).is_empty());
    }

    #[test]
    fn repeated_urls_are_reported_once_per_file() {
        let url = "https://contoso.pkgs.visualstudio.com/_packaging/x/pypi/simple/";
        let dir = repo(&[("pip.conf", &format!("index-url = {url}\nextra-index-url = {url}\n"))]);
        let findings = scan_repository(dir.path());
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].line, 1);
    }

    #[test]
    fn only_workflows_that_restore_packages_are_checked() {
        let dir = repo(&[(".npmrc", "registry=https://contoso.pkgs.visualstudio.com/_packaging/web/npm/registry/\n")]);
        let source = dir.path().join("agents").join("agent.md");

        assert!(collect_warnings(&front_matter(""), &source).is_empty());

        let warnings = collect_warnings(&front_matter("runtimes:\n  node: true\n"), &source);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("`.npmrc:1`"), "{}", warnings[0]);
        assert!(warnings[0].contains("grant the feed under `permissions.packages`"), "{}", warnings[0]);

        let warnings = collect_warnings(
            &front_matter(
                "permissions:\n  packages:\n    feeds:\n      - feed: web\n        upstream: allow\n        \
                 protocols: [npm]\n",
            ),
            &source,
        );
        assert_eq!(warnings.len(), 1);
        assert!(!warnings[0].contains("grant the feed"), "{}", warnings[0]);
    }

    fn grants(yaml: &str) -> crate::compile::types::PackagesPermissionConfig {
        front_matter(&format!("permissions:\n  packages:\n    identity-role: reader\n    feeds:\n{yaml}"))
            .permissions
            .unwrap()
            .packages
            .unwrap()
    }

    fn reasons(npmrc: &str, packages: Option<&crate::compile::types::PackagesPermissionConfig>) -> Vec<UngrantedReason> {
        let dir = repo(&[(".npmrc", npmrc)]);
        scan_repository_grants(dir.path(), packages)
            .into_iter()
            .map(|finding| finding.reason)
            .collect()
    }

    const GRANTS: &str = "      - organization: contoso\n        project: Eng\n        feed: web\n        \
         protocols: [npm]\n      - organization: contoso\n        feed: shared\n        view: Release\n        \
         protocols: [npm, pypi]\n";

    #[test]
    fn urls_the_grants_cover_are_not_reported() {
        let packages = grants(GRANTS);
        for url in [
            "https://pkgs.dev.azure.com/contoso/Eng/_packaging/web/npm/registry/",
            // Case-insensitive identifiers and a base URL without trailing slash.
            "https://pkgs.dev.azure.com/CONTOSO/eng/_packaging/WEB/npm/registry",
            "https://pkgs.dev.azure.com/contoso/_packaging/shared@Release/npm/registry/",
            "https://pkgs.dev.azure.com/contoso/_packaging/shared@release/pypi/simple/",
            // A GUID cannot be compared until resolve-feeds runs: never reported.
            "https://pkgs.dev.azure.com/contoso/_packaging/0f8fad5b-d9cb-469f-a165-70867728950e/npm/registry/",
        ] {
            assert!(reasons(&format!("registry={url}\n"), Some(&packages)).is_empty(), "{url}");
        }
    }

    #[test]
    fn each_refusal_reason_mirrors_the_proxy() {
        let packages = grants(GRANTS);
        let cases = [
            (
                "https://pkgs.dev.azure.com/contoso/Eng/_packaging/other/npm/registry/",
                UngrantedReason::FeedNotGranted,
            ),
            // An organization-scoped grant does not cover a project-scoped URL.
            (
                "https://pkgs.dev.azure.com/contoso/Eng/_packaging/shared@Release/npm/registry/",
                UngrantedReason::FeedNotGranted,
            ),
            (
                "https://pkgs.dev.azure.com/fabrikam/Eng/_packaging/web/npm/registry/",
                UngrantedReason::FeedNotGranted,
            ),
            (
                "https://pkgs.dev.azure.com/contoso/_packaging/shared/npm/registry/",
                UngrantedReason::ViewNotGranted { view: "Release".to_string() },
            ),
            (
                "https://pkgs.dev.azure.com/contoso/Eng/_packaging/web/pypi/simple/",
                UngrantedReason::ProtocolNotGranted { protocol: "pypi" },
            ),
            (
                "https://pkgs.dev.azure.com/contoso/Eng/_packaging/web/nuget/v2/",
                UngrantedReason::UnsupportedRoute,
            ),
        ];
        for (url, expected) in cases {
            assert_eq!(reasons(&format!("registry={url}\n"), Some(&packages)), vec![expected], "{url}");
        }
    }

    #[test]
    fn a_grant_without_organization_matches_any_organization_and_paths_are_decoded() {
        let packages = grants(
            "      - project: My Project\n        feed: web\n        upstream: allow\n        protocols: [npm]\n",
        );
        let url = "https://pkgs.dev.azure.com/whatever/My%20Project/_packaging/web/npm/registry/";
        assert!(reasons(&format!("registry={url}\n"), Some(&packages)).is_empty());
    }

    #[test]
    fn without_permissions_packages_every_feed_url_is_reported() {
        let dir = repo(&[(
            "NuGet.Config",
            "<add value=\"https://pkgs.dev.azure.com/contoso/_packaging/web/nuget/v3/index.json\" />\n",
        )]);
        let source = dir.path().join("agent.md");
        let warnings = collect_warnings(&front_matter("runtimes:\n  dotnet: true\n"), &source);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("`NuGet.Config:1` points at Azure Artifacts feed `contoso/web`"), "{}", warnings[0]);
        assert!(warnings[0].contains("`permissions.packages` is not configured"), "{}", warnings[0]);
    }

    #[test]
    fn messages_name_the_file_feed_and_fix() {
        let packages = grants(GRANTS);
        let dir = repo(&[(
            ".npmrc",
            "registry=https://pkgs.dev.azure.com/contoso/Eng/_packaging/web/pypi/simple/\n",
        )]);
        let finding = &scan_repository_grants(dir.path(), Some(&packages))[0];
        let message = finding.message();
        assert!(message.contains("`.npmrc:1` points at Azure Artifacts feed `contoso/Eng/web`"), "{message}");
        assert!(message.contains("`protocol-not-granted`"), "{message}");
        assert!(message.contains("Add `pypi` to the grant's `protocols`"), "{message}");
    }

    #[test]
    fn a_source_outside_a_git_repository_is_not_scanned() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".npmrc"),
            "registry=https://contoso.pkgs.visualstudio.com/_packaging/web/npm/registry/\n",
        )
        .unwrap();
        let fm = front_matter("runtimes:\n  node: true\n");
        let findings = check_source(&fm, &dir.path().join("agent.md"));
        assert!(findings.legacy.is_empty() && findings.ungranted.is_empty());
    }
}
