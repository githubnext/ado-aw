//! Runtime implementations for the ado-aw compiler.
//!
//! Runtimes are language toolchains installed before the agent runs
//! (e.g., Lean 4, Python, Node.js, .NET, and in future: Go, etc.).
//!
//! Unlike `tools/` (agent capabilities like edit, bash, memory) or
//! `safe_outputs/` (MCP tools that serialize to NDJSON), runtimes are
//! execution environments the compiler auto-installs via pipeline steps.
//!
//! Aligned with gh-aw's `runtimes:` front matter field.

pub mod dotnet;
pub mod lean;
pub mod node;
pub mod python;

use serde::Deserialize;

use crate::compile::types::{FrontMatter, PackageFeedGrant, PackageProtocol};

/// Public package-registry hosts per protocol.
///
/// Only the registry API hosts are listed; toolchain download hosts (for
/// example `nodejs.org`) stay reachable because toolchains are installed
/// outside the sandbox. Content CDNs covered by core wildcards (such as
/// `*.blob.core.windows.net`) cannot be removed individually, but they are
/// unusable without the registry API.
pub fn public_registry_hosts(protocol: PackageProtocol) -> &'static [&'static str] {
    match protocol {
        PackageProtocol::Pypi => &[
            "pypi.org",
            "pypi.python.org",
            "files.pythonhosted.org",
            "*.pythonhosted.org",
        ],
        PackageProtocol::Npm => &[
            "registry.npmjs.org",
            "registry.npmjs.com",
            "registry.yarnpkg.com",
            "repo.yarnpkg.com",
        ],
        PackageProtocol::Nuget => &[
            "api.nuget.org",
            "nuget.org",
            "azuresearch-usnc.nuget.org",
            "azuresearch-ussc.nuget.org",
        ],
        PackageProtocol::Cargo => &["crates.io", "index.crates.io", "static.crates.io"],
    }
}

/// Every public registry host an enabled runtime asks to block.
pub fn blocked_public_registry_hosts(front_matter: &FrontMatter) -> Vec<&'static str> {
    let Some(runtimes) = front_matter.runtimes.as_ref() else {
        return Vec::new();
    };
    let mut protocols = Vec::new();
    if runtimes.python.as_ref().is_some_and(|config| {
        config.is_enabled() && config.public_registry() == PublicRegistry::Block
    }) {
        protocols.push(PackageProtocol::Pypi);
    }
    if runtimes.node.as_ref().is_some_and(|config| {
        config.is_enabled() && config.public_registry() == PublicRegistry::Block
    }) {
        protocols.push(PackageProtocol::Npm);
    }
    if runtimes.dotnet.as_ref().is_some_and(|config| {
        config.is_enabled() && config.public_registry() == PublicRegistry::Block
    }) {
        protocols.push(PackageProtocol::Nuget);
    }
    protocols
        .into_iter()
        .flat_map(|protocol| public_registry_hosts(protocol).iter().copied())
        .collect()
}

/// Percent-encode a URL path segment (RFC 3986 unreserved characters kept).
fn encode_path_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// Canonical package-source URL for a granted feed.
///
/// Always the canonical `pkgs.dev.azure.com` form: the package proxy
/// intercepts that host, so a client configured with this URL reaches the
/// feed through the credential-isolated path. `current_organization` is the
/// organization inferred at compile time, used when the grant names none.
pub fn feed_source_url(
    grant: &PackageFeedGrant,
    protocol: PackageProtocol,
    current_organization: Option<&str>,
) -> anyhow::Result<String> {
    let organization = match (grant.organization.as_deref(), current_organization) {
        (Some(organization), _) | (None, Some(organization)) => organization,
        (None, None) => anyhow::bail!(
            "permissions.packages feed '{}' has no `organization`, and the current Azure \
             DevOps organization could not be inferred from the git remote. Set \
             `organization:` on the feed.",
            grant.handle()
        ),
    };
    let mut url = format!("https://pkgs.dev.azure.com/{}/", encode_path_segment(organization));
    if let Some(project) = grant.project.as_deref() {
        url.push_str(&encode_path_segment(project));
        url.push('/');
    }
    url.push_str("_packaging/");
    url.push_str(&encode_path_segment(&grant.feed));
    if let Some(view) = grant.view.as_deref() {
        url.push('@');
        url.push_str(&encode_path_segment(view));
    }
    url.push_str(match protocol {
        PackageProtocol::Npm => "/npm/registry/",
        PackageProtocol::Pypi => "/pypi/simple/",
        PackageProtocol::Nuget => "/nuget/v3/index.json",
        PackageProtocol::Cargo => "/Cargo/index/",
    });
    Ok(url)
}

/// Resolve a runtime's `feed` handle to the granted feed's source URL.
pub(crate) fn selected_feed_url(
    ctx: &crate::compile::extensions::CompileContext,
    runtime: &str,
    handle: &str,
    protocol: PackageProtocol,
) -> anyhow::Result<String> {
    let grant = ctx
        .front_matter
        .permissions
        .as_ref()
        .and_then(|permissions| permissions.packages.as_ref())
        .and_then(|packages| packages.feed(handle))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "runtimes.{runtime}.feed refers to feed '{handle}', which \
                 `permissions.packages.feeds` does not declare"
            )
        })?;
    if !grant.allows(protocol) {
        anyhow::bail!(
            "runtimes.{runtime}.feed refers to feed '{handle}', which does not grant the {} \
             protocol",
            protocol.as_str()
        );
    }
    feed_source_url(grant, protocol, ctx.ado_org())
}

/// Whether an ecosystem's public registry stays reachable from the agent.
///
/// `block` removes the ecosystem's public registry hosts from the AWF
/// allowlist so the agent can only restore through granted internal feeds.
/// Public hosts are never redirected to a feed at the network layer.
#[derive(Debug, Deserialize, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PublicRegistry {
    #[default]
    Allow,
    Block,
}

/// A runtime's reference to a `permissions.packages` feed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackageFeedSelection<'a> {
    /// Runtime key under `runtimes:` (`python`, `node`, `dotnet`).
    pub runtime: &'static str,
    /// Protocol the runtime's package manager speaks.
    pub protocol: PackageProtocol,
    /// Feed handle (`permissions.packages.feeds[].name`).
    pub handle: &'a str,
}

/// Every enabled runtime that selects a `permissions.packages` feed.
pub fn package_feed_selections(front_matter: &FrontMatter) -> Vec<PackageFeedSelection<'_>> {
    let Some(runtimes) = front_matter.runtimes.as_ref() else {
        return Vec::new();
    };
    let mut selections = Vec::new();
    if let Some(handle) = runtimes
        .python
        .as_ref()
        .filter(|config| config.is_enabled())
        .and_then(python::PythonRuntimeConfig::feed)
    {
        selections.push(PackageFeedSelection {
            runtime: "python",
            protocol: PackageProtocol::Pypi,
            handle,
        });
    }
    if let Some(handle) = runtimes
        .node
        .as_ref()
        .filter(|config| config.is_enabled())
        .and_then(node::NodeRuntimeConfig::feed)
    {
        selections.push(PackageFeedSelection {
            runtime: "node",
            protocol: PackageProtocol::Npm,
            handle,
        });
    }
    if let Some(handle) = runtimes
        .dotnet
        .as_ref()
        .filter(|config| config.is_enabled())
        .and_then(dotnet::DotnetRuntimeConfig::feed)
    {
        selections.push(PackageFeedSelection {
            runtime: "dotnet",
            protocol: PackageProtocol::Nuget,
            handle,
        });
    }
    selections
}

/// Reject a runtime that names both a granted `feed` and another source.
pub(crate) fn validate_feed_exclusivity(
    runtime: &str,
    feed: Option<&str>,
    feed_url: Option<&str>,
    config: Option<&str>,
) -> anyhow::Result<()> {
    if feed.is_some() && (feed_url.is_some() || config.is_some()) {
        anyhow::bail!(
            "runtimes.{runtime}: 'feed' cannot be combined with 'feed-url' or 'config'. \
             `feed` selects a `permissions.packages` feed; use one source."
        );
    }
    Ok(())
}

/// Compile warning for a `public-registry: block` that leaves the agent with a
/// narrower package source than the author likely expects.
///
/// With the public registry blocked, packages arrive only through the selected
/// feed. A feed with `upstream: deny` and no `view` serves only packages that
/// are already cached, so anything not yet saved from upstream fails to
/// restore.
pub(crate) fn public_registry_warning(
    front_matter: &FrontMatter,
    runtime: &str,
    public_registry: PublicRegistry,
    feed: Option<&str>,
    other_source: bool,
) -> Option<String> {
    if public_registry != PublicRegistry::Block {
        return None;
    }
    let Some(handle) = feed else {
        return (!other_source).then(|| {
            format!(
                "runtimes.{runtime}.public-registry: block removes the public registry, but \
                 runtimes.{runtime} selects no other source. The agent can restore only from \
                 sources configured in the repository."
            )
        });
    };
    let grant = front_matter
        .permissions
        .as_ref()
        .and_then(|permissions| permissions.packages.as_ref())
        .and_then(|packages| packages.feed(handle))?;
    (grant.upstream == crate::compile::types::PackageUpstream::Deny && grant.view.is_none())
        .then(|| {
            format!(
                "runtimes.{runtime}.public-registry: block leaves feed '{handle}' as the only \
                 source, and that feed has `upstream: deny` with no `view`. Only packages \
                 already cached in the feed can be restored; set `upstream: allow` or pin a \
                 `view` that contains the packages the agent needs."
            )
        })
}

/// Compile warning for a runtime package-source setting.
///
/// The Azure Pipelines package-authenticate tasks publish their credential as
/// a non-secret job variable or an agent-visible file, so the compiler no
/// longer runs them in the Agent job. The source setting still selects the
/// feed, but requests from the agent carry no credential.
pub(crate) fn unauthenticated_feed_warning(field: &str) -> String {
    format!(
        "{field} selects the package source only. The agent has no feed credential, so \
         feeds that require authentication (including Azure Artifacts) will reject \
         requests from inside the sandbox."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn front_matter(packages: &str) -> FrontMatter {
        crate::compile::parse_markdown(&format!(
            "---\nname: t\ndescription: x\npermissions:\n  packages:\n    identity-role: reader\n    feeds:\n{packages}---\n"
        ))
        .unwrap()
        .0
    }

    #[test]
    fn feed_is_exclusive_with_feed_url_and_config() {
        validate_feed_exclusivity("node", Some("internal"), None, None).unwrap();
        validate_feed_exclusivity("node", None, Some("https://x.test/"), Some("a")).unwrap();
        for (feed_url, config) in [(Some("https://x.test/"), None), (None, Some(".npmrc"))] {
            let err = validate_feed_exclusivity("node", Some("internal"), feed_url, config)
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("runtimes.node: 'feed' cannot be combined with 'feed-url' or 'config'"),
                "{err}"
            );
        }
    }

    #[test]
    fn each_runtime_rejects_feed_with_another_source() {
        for (runtime, other) in [
            ("python", "feed-url: https://packages.example.test/simple/"),
            ("node", "config: .npmrc"),
            ("dotnet", "config: nuget.config"),
        ] {
            let source = format!(
                "---\nname: t\ndescription: x\npermissions:\n  packages:\n    feeds:\n      \
                 - feed: internal\n        organization: contoso\n        upstream: allow\n        \
                 protocols: [npm, pypi, nuget]\nruntimes:\n  {runtime}:\n    feed: internal\n    \
                 {other}\n---\n"
            );
            let (fm, _) = crate::compile::parse_markdown(&source).unwrap();
            let runtimes = fm.runtimes.as_ref().unwrap();
            let ctx = crate::compile::extensions::CompileContext::for_test(&fm);
            use crate::compile::extensions::CompilerExtension;
            let err = match runtime {
                "python" => python::extension::PythonExtension::new(runtimes.python.clone().unwrap())
                    .declarations(&ctx),
                "node" => node::extension::NodeExtension::new(runtimes.node.clone().unwrap())
                    .declarations(&ctx),
                _ => dotnet::extension::DotnetExtension::new(runtimes.dotnet.clone().unwrap())
                    .declarations(&ctx),
            }
            .unwrap_err()
            .to_string();
            assert!(
                err.contains(&format!("runtimes.{runtime}: 'feed' cannot be combined")),
                "{runtime}: {err}"
            );
        }
    }
    #[test]
    fn public_registry_warning_is_silent_unless_blocked() {
        let fm = front_matter("      - feed: internal\n        protocols: [npm]\n");
        assert!(
            public_registry_warning(&fm, "node", PublicRegistry::Allow, Some("internal"), false)
                .is_none()
        );
    }

    #[test]
    fn public_registry_warning_flags_a_block_without_any_source() {
        let fm = front_matter("      - feed: internal\n        protocols: [npm]\n");
        let warning =
            public_registry_warning(&fm, "node", PublicRegistry::Block, None, false).unwrap();
        assert!(warning.contains("selects no other source"), "{warning}");
        assert!(public_registry_warning(&fm, "node", PublicRegistry::Block, None, true).is_none());
    }

    #[test]
    fn public_registry_warning_flags_a_cache_only_feed() {
        let fm = front_matter(
            "      - feed: internal\n        protocols: [npm]\n      - feed: promoted\n        \
             view: Release\n        protocols: [npm]\n      - feed: open\n        upstream: allow\n        \
             protocols: [npm]\n",
        );
        let warning =
            public_registry_warning(&fm, "node", PublicRegistry::Block, Some("internal"), false)
                .unwrap();
        assert!(warning.contains("already cached"), "{warning}");
        for handle in ["promoted", "open"] {
            assert!(
                public_registry_warning(&fm, "node", PublicRegistry::Block, Some(handle), false)
                    .is_none(),
                "{handle}"
            );
        }
    }
}