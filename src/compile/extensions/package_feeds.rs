//! Package-manager wiring for credential-isolated Azure Artifacts access.
//!
//! When `permissions.packages` is configured, the `ado-proxy` engine holds the
//! feed credential and polices requests to the package host. This extension
//! installs per-tool wrappers that point *only* the package managers at the
//! engine, with trust in its interception CA scoped to that process.
//!
//! # Why wrappers rather than a sandbox-wide proxy
//!
//! The same reasoning as the `az` wrapper (see `compile/az_wrapper.rs`):
//! installing the interception CA or proxy sandbox-wide would extend the
//! engine's reach to every client, and different runtimes consult different
//! trust stores. A wrapper sets `HTTPS_PROXY` and the per-tool trust variable
//! for one process and execs the real binary, so package URLs stay canonical
//! (`pkgs.dev.azure.com`) and repository config files keep working unchanged.
//!
//! # Fail-closed property
//!
//! Routing is the boundary, not trust. A client that bypasses its wrapper
//! (for example `python -m pip`) reaches `pkgs.dev.azure.com` through Squid
//! with no credential and is refused by Azure Artifacts; it never obtains the
//! feed credential, which exists only inside the engine.

use super::{CompileContext, CompilerExtension, Declarations, ExtensionPhase};
use crate::compile::common::{
    ADO_MCP_TOKEN_SENTINEL, ADO_PROXY_CONTAINER_NAME, ADO_PROXY_LISTEN_PORT,
    ADO_PROXY_PUBLIC_CA_BUNDLE_HOST_PATH, ADO_PROXY_PUBLIC_CA_HOST_PATH, PACKAGE_WRAPPER_DIR,
};
use crate::compile::ir::step::{BashStep, Step};
use crate::compile::shell::{Binding, ShellScript};
use crate::compile::types::{PackageProtocol, PackageUpstream, PackagesPermissionConfig};
use crate::shell_script;

/// Package-manager entry points the wrapper is installed under.
pub const WRAPPED_TOOLS: &[&str] = &["npm", "npx", "pip", "pip3", "uv", "dotnet", "cargo"];

/// File name of the shared wrapper; each tool name is a symlink to it.
const WRAPPER_FILE: &str = ".ado-aw-package-wrapper";

/// File name of the Cargo token helper inside [`PACKAGE_WRAPPER_DIR`].
const CARGO_TOKEN_FILE: &str = ".ado-aw-cargo-token";

shell_script! {
    /// The wrapper each package-manager name resolves to. Written to disk by
    /// [`INSTALL_PACKAGE_WRAPPERS`] rather than run as a step.
    PACKAGE_WRAPPER {
        interpreter: Sh,
        bindings: [ENGINE_HOST, ENGINE_PORT, CA_PATH, CA_BUNDLE_PATH, WRAPPER_DIR, CARGO_TOKEN_PATH],
        externals: [PATH],
        fragments: [],
        body: r#"#!/bin/sh
# Package-manager wrapper installed by ado-aw.
#
# The agent has no package-feed credential. This wrapper routes the package
# manager it names through the ado-proxy engine, which holds the credential and
# attaches it only to read-only requests for the feeds the workflow grants.
# Every other destination is tunnelled to the normal egress proxy untouched.
#
# Generated — edits here are overwritten on every run.
set -eu

TOOL=$(basename "$0")

# Locate the real binary. `exec "$TOOL"` would re-enter this wrapper, because
# the wrapper's own directory is prepended to PATH.
REAL=""
IFS=:
for dir in $PATH; do
  case "$dir" in
    ""|"$WRAPPER_DIR") continue ;;
  esac
  if [ -x "$dir/$TOOL" ] && [ ! -d "$dir/$TOOL" ]; then
    REAL="$dir/$TOOL"
    break
  fi
done
unset IFS

if [ -z "$REAL" ]; then
  echo "ado-aw: $TOOL is not installed on this image." >&2
  exit 127
fi

HTTPS_PROXY="http://$ENGINE_HOST:$ENGINE_PORT"
export HTTPS_PROXY
https_proxy="$HTTPS_PROXY"
export https_proxy

# Trust the engine's interception certificate for this process only. Tools
# that accept an *additional* CA get the CA alone; tools that only accept a
# replacement bundle get the system roots plus the CA.
case "$TOOL" in
  npm|npx)
    NODE_EXTRA_CA_CERTS="$CA_PATH"
    export NODE_EXTRA_CA_CERTS
    # `npm audit` is a POST, which the engine refuses; skip it rather than
    # report a spurious failure.
    npm_config_audit=false
    export npm_config_audit
    ;;
  pip|pip3)
    PIP_CERT="$CA_BUNDLE_PATH"
    export PIP_CERT
    ;;
  uv|dotnet)
    SSL_CERT_FILE="$CA_BUNDLE_PATH"
    export SSL_CERT_FILE
    ;;
  cargo)
    CARGO_HTTP_PROXY="$HTTPS_PROXY"
    export CARGO_HTTP_PROXY
    CARGO_HTTP_CAINFO="$CA_BUNDLE_PATH"
    export CARGO_HTTP_CAINFO
    # Azure Artifacts Cargo feeds set `auth-required`, so Cargo refuses to
    # send a request without *some* token. Supply a non-secret placeholder;
    # the engine strips it and attaches the real credential.
    CARGO_CREDENTIAL_ALIAS_ADOAWPROXY="cargo:token-from-stdout $CARGO_TOKEN_PATH"
    export CARGO_CREDENTIAL_ALIAS_ADOAWPROXY
    CARGO_REGISTRY_GLOBAL_CREDENTIAL_PROVIDERS="adoawproxy"
    export CARGO_REGISTRY_GLOBAL_CREDENTIAL_PROVIDERS
    ;;
esac

exec "$REAL" "$@"
"#,
    }
}

shell_script! {
    /// Cargo credential helper: prints the non-secret placeholder token.
    PACKAGE_CARGO_TOKEN {
        interpreter: Sh,
        bindings: [SENTINEL],
        externals: [],
        fragments: [],
        body: r#"#!/bin/sh
# Non-secret placeholder for Cargo's auth-required registries. The ado-proxy
# engine replaces it with the real feed credential after its policy check.
printf '%s\n' "$SENTINEL"
"#,
    }
}

shell_script! {
    /// Install the package-manager wrapper and one symlink per tool name.
    ///
    /// The wrapper and helper texts are spliced as fragments because they are
    /// complete scripts, not values; both heredoc delimiters are quoted so the
    /// installing shell performs no expansion.
    INSTALL_PACKAGE_WRAPPERS {
        interpreter: Bash,
        bindings: [WRAPPER_DIR, WRAPPER_PATH, CARGO_TOKEN_PATH, TOOLS],
        externals: [],
        fragments: [wrapper, cargo_token],
        body: r#"
set -eo pipefail
mkdir -p "$WRAPPER_DIR"
cat > "$WRAPPER_PATH" << 'ADO_AW_PACKAGE_WRAPPER_EOF'
# ado-aw:fragment wrapper
ADO_AW_PACKAGE_WRAPPER_EOF
chmod 755 "$WRAPPER_PATH"
cat > "$CARGO_TOKEN_PATH" << 'ADO_AW_CARGO_TOKEN_EOF'
# ado-aw:fragment cargo_token
ADO_AW_CARGO_TOKEN_EOF
chmod 755 "$CARGO_TOKEN_PATH"
# shellcheck disable=SC2086 # TOOLS is Binding::words; unquoted expansion is the documented word-list contract.
for TOOL in $TOOLS; do
  ln -sf "$WRAPPER_PATH" "$WRAPPER_DIR/$TOOL"
done
echo "package-manager wrappers installed in $WRAPPER_DIR"
"#,
    }
}

/// Render the package-manager wrapper.
pub(crate) fn render_package_wrapper() -> String {
    ShellScript::new(&PACKAGE_WRAPPER)
        .bind_text("ENGINE_HOST", ADO_PROXY_CONTAINER_NAME)
        .bind("ENGINE_PORT", Binding::number(u64::from(ADO_PROXY_LISTEN_PORT)))
        .bind_text("CA_PATH", ADO_PROXY_PUBLIC_CA_HOST_PATH)
        .bind_text("CA_BUNDLE_PATH", ADO_PROXY_PUBLIC_CA_BUNDLE_HOST_PATH)
        .bind_text("WRAPPER_DIR", PACKAGE_WRAPPER_DIR)
        .bind_text("CARGO_TOKEN_PATH", cargo_token_path())
        .render()
}

fn cargo_token_path() -> String {
    format!("{PACKAGE_WRAPPER_DIR}/{CARGO_TOKEN_FILE}")
}

fn install_wrappers_step() -> BashStep {
    let cargo_token = ShellScript::new(&PACKAGE_CARGO_TOKEN)
        .bind_text("SENTINEL", ADO_MCP_TOKEN_SENTINEL)
        .render();
    ShellScript::new(&INSTALL_PACKAGE_WRAPPERS)
        .bind_text("WRAPPER_DIR", PACKAGE_WRAPPER_DIR)
        .bind_text("WRAPPER_PATH", format!("{PACKAGE_WRAPPER_DIR}/{WRAPPER_FILE}"))
        .bind_text("CARGO_TOKEN_PATH", cargo_token_path())
        .bind("TOOLS", Binding::words(WRAPPED_TOOLS.iter().copied()))
        .fragment("wrapper", render_package_wrapper())
        .fragment("cargo_token", cargo_token)
        .into_step("Install package-manager wrappers (ado-proxy)")
}

/// Agent-facing description of the granted feeds.
fn prompt_supplement(packages: &PackagesPermissionConfig) -> String {
    let mut lines = Vec::new();
    for feed in &packages.feeds {
        let mut location = feed
            .organization
            .as_deref()
            .map_or_else(|| "current organization".to_string(), |org| format!("`{org}`"));
        if let Some(project) = feed.project.as_deref() {
            location.push_str(&format!(", project `{project}`"));
        }
        let view = feed
            .view
            .as_deref()
            .map_or_else(String::new, |view| format!(", view `{view}` only"));
        let upstream = match feed.upstream {
            PackageUpstream::Allow => "may pull new upstream packages",
            PackageUpstream::Deny => "cached or promoted packages only",
        };
        let protocols = feed
            .protocols
            .iter()
            .map(|protocol| format!("`{}`", protocol.as_str()))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!(
            "- Feed `{}` ({location}{view}): {protocols}; {upstream}.",
            feed.feed.as_str()
        ));
    }
    let tools = if packages
        .feeds
        .iter()
        .any(|feed| feed.allows(PackageProtocol::Cargo))
    {
        "`npm`, `npx`, `pip`, `uv`, `dotnet`, and `cargo`"
    } else {
        "`npm`, `npx`, `pip`, `uv`, and `dotnet`"
    };
    format!(
        "\n\
---\n\
\n\
## Internal package feeds\n\
\n\
Azure Artifacts feeds are reachable through a credential-isolated proxy. You have no feed credential and need none: invoke {tools} directly and the proxy authenticates read-only requests to these feeds:\n\
\n\
{}\n\
\n\
Publishing, unlisting, deleting, and `npm audit` are refused. Invoke the package managers by name — `python -m pip` bypasses the proxy and is rejected by the feed. A refusal is a policy result: do not try to sign in or add credentials. If you need a feed or protocol not listed, report it as missing tooling.\n",
        lines.join("\n")
    )
}

/// Package-feed extension, enabled by `permissions.packages`.
pub struct PackageFeedsExtension {
    config: PackagesPermissionConfig,
}

impl PackageFeedsExtension {
    pub fn new(config: PackagesPermissionConfig) -> Self {
        Self { config }
    }
}

impl CompilerExtension for PackageFeedsExtension {
    fn name(&self) -> &str {
        "Package feeds"
    }

    fn phase(&self) -> ExtensionPhase {
        ExtensionPhase::Tool
    }

    fn declarations(&self, ctx: &CompileContext) -> anyhow::Result<Declarations> {
        let mut warnings = Vec::new();
        let edit_disabled = ctx
            .front_matter
            .tools
            .as_ref()
            .and_then(|tools| tools.edit)
            == Some(false);
        if edit_disabled {
            // Observed live: without `--allow-all-paths` Copilot CLI refuses
            // every npm, pip, and dotnet invocation non-interactively.
            warnings.push(
                "permissions.packages is configured but tools.edit is false. Without edit, \
                 Copilot CLI runs without path permissions and refuses package-manager \
                 commands, which write caches and temporary files outside the workspace. \
                 Leave tools.edit enabled for workflows that restore packages."
                    .to_string(),
            );
        }
        Ok(Declarations {
            agent_prepare_steps: vec![Step::Bash(install_wrappers_step())],
            awf_path_prepends: vec![PACKAGE_WRAPPER_DIR.to_string()],
            prompt_supplement: Some(prompt_supplement(&self.config)),
            warnings,
            ..Declarations::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::parse_markdown;

    fn packages(yaml: &str) -> PackagesPermissionConfig {
        let (fm, _) = parse_markdown(&format!(
            "---\nname: t\ndescription: x\npermissions:\n  packages:\n{yaml}---\n"
        ))
        .unwrap();
        fm.permissions.unwrap().packages.unwrap()
    }

    #[test]
    fn wrapper_routes_only_its_own_process_and_carries_no_credential() {
        let wrapper = render_package_wrapper();
        assert!(wrapper.starts_with("#!/bin/sh"), "{wrapper}");
        assert!(wrapper.contains(&format!("ENGINE_HOST='{ADO_PROXY_CONTAINER_NAME}'")));
        assert!(wrapper.contains(&format!("ENGINE_PORT={ADO_PROXY_LISTEN_PORT}")));
        assert!(wrapper.contains("exec \"$REAL\" \"$@\""));
        for forbidden in ["SYSTEM_ACCESSTOKEN", "SC_PACKAGES_TOKEN", "System.AccessToken"] {
            assert!(!wrapper.contains(forbidden), "{forbidden} leaked into wrapper");
        }
    }

    #[test]
    fn install_step_links_every_tool_to_the_shared_wrapper() {
        let step = install_wrappers_step();
        for tool in WRAPPED_TOOLS {
            assert!(step.script.contains(tool), "{tool} missing: {}", step.script);
        }
        assert!(step.script.contains("ln -sf \"$WRAPPER_PATH\" \"$WRAPPER_DIR/$TOOL\""));
        assert!(step.script.contains(ADO_MCP_TOKEN_SENTINEL));
    }

    #[test]
    fn declarations_prepend_the_wrapper_dir_and_describe_the_feeds() {
        let config = packages(
            "    feeds:\n      - feed: internal\n        project: Eng\n        view: Release\n        \
             protocols: [npm, cargo]\n",
        );
        let fm = crate::compile::parse_markdown("---\nname: t\ndescription: x\n---\n")
            .unwrap()
            .0;
        let decl = PackageFeedsExtension::new(config)
            .declarations(&CompileContext::for_test(&fm))
            .unwrap();
        assert_eq!(decl.awf_path_prepends, vec![PACKAGE_WRAPPER_DIR.to_string()]);
        let prompt = decl.prompt_supplement.unwrap();
        assert!(prompt.contains("Feed `internal` (current organization, project `Eng`, view `Release` only): `npm`, `cargo`"), "{prompt}");
        assert!(prompt.contains("`cargo`"));
        assert!(decl.warnings.is_empty(), "{:?}", decl.warnings);
    }

    #[test]
    fn warns_when_edit_is_disabled() {
        let config = packages("    feeds:\n      - feed: internal\n        upstream: allow\n        protocols: [npm]\n");
        let fm = crate::compile::parse_markdown(
            "---\nname: t\ndescription: x\ntools:\n  edit: false\n---\n",
        )
        .unwrap()
        .0;
        let decl = PackageFeedsExtension::new(config)
            .declarations(&CompileContext::for_test(&fm))
            .unwrap();
        assert_eq!(decl.warnings.len(), 1);
        assert!(decl.warnings[0].contains("tools.edit is false"), "{:?}", decl.warnings);
    }
}
