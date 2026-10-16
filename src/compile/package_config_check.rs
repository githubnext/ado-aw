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

/// Scan the well-known package config files under `root`.
pub fn scan_repository(root: &Path) -> Vec<LegacyFeedUrl> {
    let pattern = legacy_url_pattern();
    let root_entries: Vec<String> = std::fs::read_dir(root)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
                .filter_map(|entry| entry.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();

    let mut findings = Vec::new();
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
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        findings.extend(scan_content(&relative, &content, &pattern));
    }
    findings
}

/// Findings for the repository containing `source_path`, or nothing when the
/// workflow cannot restore packages or the source is not in a git repository.
pub fn check_source(front_matter: &FrontMatter, source_path: &Path) -> Vec<LegacyFeedUrl> {
    if !applies_to(front_matter) {
        return Vec::new();
    }
    let absolute = std::path::absolute(source_path).unwrap_or_else(|_| source_path.to_path_buf());
    super::common::find_git_root(&absolute)
        .map(|root| scan_repository(&root))
        .unwrap_or_default()
}

/// Compile-time warnings for the repository containing `source_path`.
pub fn collect_warnings(front_matter: &FrontMatter, source_path: &Path) -> Vec<String> {
    let packages_configured = front_matter
        .permissions
        .as_ref()
        .is_some_and(|permissions| permissions.packages.is_some());
    check_source(front_matter, source_path)
        .iter()
        .map(|finding| finding.message(packages_configured))
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

    #[test]
    fn a_source_outside_a_git_repository_is_not_scanned() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".npmrc"),
            "registry=https://contoso.pkgs.visualstudio.com/_packaging/web/npm/registry/\n",
        )
        .unwrap();
        let fm = front_matter("runtimes:\n  node: true\n");
        assert!(check_source(&fm, &dir.path().join("agent.md")).is_empty());
    }
}
