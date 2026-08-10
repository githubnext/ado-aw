//! The `az` wrapper installed into the agent sandbox.
//!
//! The agent runs stock `az`. It is *redirected*, not rewritten: the wrapper
//! sets three environment variables and execs the real binary.
//!
//! # Why environment rather than argument rewriting
//!
//! An earlier design rewrote `--organization` to point at a broker hostname.
//! That was both harder and weaker:
//!
//! - the organization can be given as `--organization`, `--org`,
//!   `AZURE_DEVOPS_ORG`, or a stored `az devops configure --defaults` value,
//!   so rewriting means enumerating every form and staying correct as the CLI
//!   evolves — a miss silently escapes the policy;
//! - a non-canonical hostname has no interception leaf and matches no
//!   catalogued route, since both are keyed to `dev.azure.com`.
//!
//! Pointing `HTTPS_PROXY` at the engine instead makes every form work
//! identically, because the redirect happens below the CLI's own
//! configuration. Verified with real `az` 2.86: `az devops project list
//! --organization https://dev.azure.com/contoso` reached the engine through
//! `CONNECT`, verified the intercepted certificate, and the request arriving
//! upstream carried the *injected* bearer while the sentinel the CLI held
//! never appeared there.
//!
//! # Scope of trust
//!
//! `REQUESTS_CA_BUNDLE` is set for this process only. Python's `requests`
//! bundles its own `certifi` roots and ignores the OS trust store, so a
//! system-wide install would not help `az` anyway — and per-process trust
//! keeps the interception certificate off every other client. Trust here is an
//! *availability* control: enforcement comes from routing, so a client that
//! declines the certificate fails closed rather than escaping the policy.

use super::common::{AZ_WRAPPER_CA_PATH, AZ_WRAPPER_DIR, az_allowed_groups};
use super::shell::{Binding, ShellScript};
use crate::ado_proxy::catalog::Capability;
use crate::shell_script;

shell_script! {
    /// Wrapper script the agent invokes as `az`. Written to disk by
    /// `extensions/azure_cli.rs::install_az_wrapper_step` rather than run as a
    /// step, so this is an `Sh` script rendered to a String; every value that
    /// used to be interpolated with `format!` is now a validated binding, and
    /// the body is the shell exactly as it will run.
    AZ_WRAPPER {
        interpreter: Sh,
        bindings: [ALLOWED_GROUPS, ALLOWED_DISPLAY, ENGINE_HOST, ENGINE_PORT, CA_PATH, SENTINEL, WRAPPER_DIR],
        externals: [TMPDIR, PATH],
        fragments: [],
        body: r#"#!/bin/sh
# Azure CLI wrapper installed by ado-aw.
#
# The agent has no Azure DevOps credential. This wrapper points `az` at the
# ado-proxy policy engine, which holds the credential and serves only the
# operations in its versioned read-only catalog.
#
# Generated — edits here are overwritten on every run.
set -eu

# Refuse command groups whose traffic the policy does not describe. They would
# otherwise fail somewhere far less legible: no Azure credential is present, so
# `az vm` or `az storage` would surface an authentication error that looks like
# a broken pipeline rather than a deliberate boundary.
AZ_GROUP="${1:-}"
case " $ALLOWED_GROUPS " in
  *" $AZ_GROUP "*) ;;
  *)
    case "$AZ_GROUP" in
      ""|-h|--help|--version|-v)
        # Informational invocations touch no network; let them through.
        ;;
      *)
        echo "ado-aw: 'az $AZ_GROUP' is not available to this agent." >&2
        echo "" >&2
        echo "This workflow reaches Azure DevOps through a policy proxy that" >&2
        echo "serves read-only operations for the current project. Available" >&2
        echo "command groups: $ALLOWED_DISPLAY." >&2
        echo "" >&2
        echo "To act outside that boundary, use a safe output instead:" >&2
        echo "https://github.com/githubnext/ado-aw/blob/main/docs/safe-outputs.md" >&2
        exit 1
        ;;
    esac
    ;;
esac

# Route Azure DevOps traffic through the policy engine. This is what makes the
# redirect independent of how the organization was specified — --organization,
# --org, AZURE_DEVOPS_ORG and stored defaults all resolve to the same canonical
# host, and that host is what gets intercepted.
HTTPS_PROXY="http://$ENGINE_HOST:$ENGINE_PORT"
export HTTPS_PROXY
https_proxy="$HTTPS_PROXY"
export https_proxy

# Trust the engine's interception certificate for this process only. Python's
# requests ignores the OS trust store, so this variable — not the system CA
# bundle — is what `az` actually consults.
REQUESTS_CA_BUNDLE="$CA_PATH"
export REQUESTS_CA_BUNDLE

# Azure CLI writes extension metadata, command indexes and defaults beneath its
# config directory. The rootless AWF agent cannot write the runner user's
# default ~/.azure, so establish a private, writable sandbox-local default.
# Honour an explicit caller override for tests and advanced use.
AZURE_CONFIG_DIR="${AZURE_CONFIG_DIR:-${TMPDIR:-/tmp}/ado-aw-az-config}"
export AZURE_CONFIG_DIR
mkdir -p "$AZURE_CONFIG_DIR"
if [ ! -w "$AZURE_CONFIG_DIR" ]; then
  echo "ado-aw: Azure CLI config directory is not writable: $AZURE_CONFIG_DIR" >&2
  exit 1
fi

# A non-secret placeholder. `az` requires *some* credential to attempt a call;
# the engine strips whatever the client sent and attaches the real bearer only
# after a complete allow decision.
AZURE_DEVOPS_EXT_PAT="$SENTINEL"
export AZURE_DEVOPS_EXT_PAT

# Locate the real binary. `exec az` would re-enter this wrapper, because the
# wrapper's own directory is prepended to PATH.
AZ_REAL=""
IFS=:
for dir in $PATH; do
  case "$dir" in
    ""|"$WRAPPER_DIR") continue ;;
  esac
  if [ -x "$dir/az" ]; then
    AZ_REAL="$dir/az"
    break
  fi
done
unset IFS

if [ -z "$AZ_REAL" ]; then
  echo "ado-aw: the Azure CLI is not installed on this image." >&2
  exit 127
fi

exec "$AZ_REAL" "$@"
"#,
    }
}

/// Build the wrapper as a typed [`ShellScript`], with each interpolated value
/// carried as a validated [`Binding`] rather than a `format!` substitution.
///
/// The list of `az` command groups the wrapper advertises is derived from
/// [`az_allowed_groups`] rather than hand-maintained, so a capability the
/// policy does not grant cannot be advertised to the agent. `allowed_display`
/// is a comma-joined form of the same list; the deny-branch message consumes
/// it verbatim, so the two shapes come from the same source and cannot drift
/// per site.
pub(crate) fn az_wrapper_script(
    engine_host: &str,
    connect_port: u16,
    sentinel: &str,
    capabilities: &[Capability],
) -> ShellScript {
    let groups = az_allowed_groups(capabilities);
    let allowed_display = if groups.is_empty() {
        "none".to_string()
    } else {
        groups.join(", ")
    };

    ShellScript::new(&AZ_WRAPPER)
        .bind("ALLOWED_GROUPS", Binding::words(groups.iter().copied()))
        .text("ALLOWED_DISPLAY", allowed_display)
        .text("ENGINE_HOST", engine_host)
        .bind("ENGINE_PORT", Binding::number(connect_port as u64))
        .text("CA_PATH", AZ_WRAPPER_CA_PATH)
        .text("SENTINEL", sentinel)
        .text("WRAPPER_DIR", AZ_WRAPPER_DIR)
}

/// Render the wrapper script.
///
/// `engine_host` is the policy engine's container name, which AWF registers in
/// the agent's `/etc/hosts` when it attaches the container to the internal
/// network. `capabilities` are the ones the policy actually grants, so the
/// wrapper refuses a command group the engine would refuse anyway — with an
/// explanation, rather than an opaque `403` several layers down.
#[allow(dead_code)]
pub fn render_az_wrapper(
    engine_host: &str,
    connect_port: u16,
    sentinel: &str,
    capabilities: &[Capability],
) -> String {
    az_wrapper_script(engine_host, connect_port, sentinel, capabilities).render()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::common::ADO_MCP_TOKEN_SENTINEL;

    fn wrapper() -> String {
        render_az_wrapper(
            "awmg-ado-proxy",
            11080,
            ADO_MCP_TOKEN_SENTINEL,
            Capability::ALL,
        )
    }

    fn wrapper_script() -> ShellScript {
        az_wrapper_script(
            "awmg-ado-proxy",
            11080,
            ADO_MCP_TOKEN_SENTINEL,
            Capability::ALL,
        )
    }

    #[test]
    fn routes_azure_devops_traffic_through_the_engine() {
        let s = wrapper_script();
        // The producer supplied the engine host and port; the body glues them
        // into the HTTPS_PROXY URL. Asserting on the bindings is a
        // strengthening: it verifies the values reached the prelude, rather
        // than that the concatenated URL happens to appear as a substring.
        assert_eq!(s.binding("ENGINE_HOST").unwrap().rhs(), "'awmg-ado-proxy'");
        assert_eq!(s.binding("ENGINE_PORT").unwrap().rhs(), "11080");
        let script = s.render();
        assert!(
            script.contains(r#"HTTPS_PROXY="http://$ENGINE_HOST:$ENGINE_PORT""#),
            "the body must glue the two together into the proxy URL: {script}"
        );
        assert!(
            script.contains("export HTTPS_PROXY") && script.contains("export https_proxy"),
            "both spellings matter: tooling reads one or the other"
        );
    }

    #[test]
    fn trusts_the_interception_certificate_for_this_process_only() {
        let s = wrapper_script();
        assert_eq!(
            s.binding("CA_PATH").unwrap().rhs(),
            format!("'{AZ_WRAPPER_CA_PATH}'")
        );
        let script = s.render();
        assert!(script.contains(r#"REQUESTS_CA_BUNDLE="$CA_PATH""#));
        // A system-wide install would not help: Python's requests uses its own
        // certifi bundle. It would also widen trust beyond this one client.
        assert!(!script.contains("update-ca-certificates"));
        assert!(!script.contains("/usr/local/share/ca-certificates"));
    }

    #[test]
    fn provides_a_writable_azure_cli_config_directory() {
        let script = wrapper();
        assert!(script.contains(
            "AZURE_CONFIG_DIR=\"${AZURE_CONFIG_DIR:-${TMPDIR:-/tmp}/ado-aw-az-config}\""
        ));
        assert!(script.contains("mkdir -p \"$AZURE_CONFIG_DIR\""));
        assert!(script.contains("if [ ! -w \"$AZURE_CONFIG_DIR\" ]"));
        // A caller may deliberately isolate invocations further; the wrapper
        // supplies a safe default rather than overriding one.
        assert!(script.contains("${AZURE_CONFIG_DIR:-"));
    }

    #[test]
    fn carries_a_sentinel_rather_than_a_credential() {
        let s = wrapper_script();
        assert_eq!(
            s.binding("SENTINEL").unwrap().rhs(),
            format!("'{ADO_MCP_TOKEN_SENTINEL}'"),
            "the wrapper's PAT value must be the compiler-supplied sentinel"
        );
        let script = s.render();
        assert!(script.contains(r#"AZURE_DEVOPS_EXT_PAT="$SENTINEL""#));
        assert!(
            !script.contains("SC_READ_TOKEN") && !script.contains("System.AccessToken"),
            "no real credential may appear in an agent-readable file: {script}"
        );
    }

    #[test]
    fn does_not_rewrite_how_the_organization_was_specified() {
        // The redirect happens below the CLI's configuration, so every form
        // works without the wrapper having to know about any of them. Touching
        // them would reintroduce the enumeration problem this design avoids.
        //
        // Comments are stripped first: the rationale above legitimately names
        // these flags, and asserting on prose would make the guard vacuous.
        let script = wrapper();
        let code: String = script
            .lines()
            .filter(|line| !line.trim_start().starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n");
        for form in ["--organization", "--org", "AZURE_DEVOPS_ORG"] {
            assert!(
                !code.contains(form),
                "the wrapper must not interpret {form}: {code}"
            );
        }
    }

    #[test]
    fn refuses_command_groups_outside_the_policed_surface() {
        let script = wrapper();
        for capability in Capability::ALL {
            if let Some(group) = capability.az_command_group() {
                assert!(
                    script.contains(group),
                    "{group} is catalogued and must be permitted"
                );
            }
        }
        assert!(script.contains("is not available to this agent"));
        // An actionable message: a bare denial invites the agent to retry the
        // same call, or to conclude the pipeline is broken.
        assert!(script.contains("safe-outputs.md"));
    }

    #[test]
    fn advertises_only_what_the_policy_actually_grants() {
        // `az artifacts` was briefly permitted while no catalogued operation
        // backed it, so the call passed the wrapper and was refused by the
        // engine. The allow-list is now derived from the granted capabilities,
        // and reaches the prelude verbatim as `ALLOWED_GROUPS` /
        // `ALLOWED_DISPLAY` — not as a substring smuggled through the body.
        let s = wrapper_script();
        assert!(!s.binding("ALLOWED_GROUPS").unwrap().rhs().contains("artifacts"));
        assert!(
            !s.binding("ALLOWED_DISPLAY")
                .unwrap()
                .rhs()
                .contains("artifacts")
        );

        // Narrowing the policy narrows the wrapper with it.
        let repos_only = az_wrapper_script(
            "awmg-ado-proxy",
            11080,
            ADO_MCP_TOKEN_SENTINEL,
            &[Capability::Discovery, Capability::Repos],
        );
        let groups = repos_only.binding("ALLOWED_GROUPS").unwrap().rhs();
        assert!(groups.contains("repos"), "repos must appear: {groups}");
        for absent in ["devops", "boards", "pipelines"] {
            assert!(
                !groups.contains(absent),
                "{absent} is not granted and must not be advertised: {groups}"
            );
        }
    }

    #[test]
    fn rest_stays_available_whatever_the_capabilities() {
        // `az rest` is contained by the catalog, not by this list: measured
        // against a live engine it completed a catalogued read and was refused
        // 403 for a denied route family and for a POST. Excluding it would
        // also contradict `az devops invoke`, which reaches the same surface.
        let full = wrapper_script();
        assert!(full.binding("ALLOWED_GROUPS").unwrap().rhs().contains("rest"));
        let narrow = az_wrapper_script(
            "awmg-ado-proxy",
            11080,
            ADO_MCP_TOKEN_SENTINEL,
            &[Capability::Discovery],
        );
        assert!(narrow.binding("ALLOWED_GROUPS").unwrap().rhs().contains("rest"));
    }

    #[test]
    fn execs_the_real_binary_without_re_entering_itself() {
        let s = wrapper_script();
        assert_eq!(s.binding("WRAPPER_DIR").unwrap().rhs(), format!("'{AZ_WRAPPER_DIR}'"));
        let script = s.render();
        // The wrapper directory is prepended to PATH, so a bare `exec az`
        // would loop until the process ran out of file descriptors. The body
        // consults `$WRAPPER_DIR` in a `case` pattern to skip its own dir.
        assert!(script.contains(r#"""|"$WRAPPER_DIR") continue"#));
        assert!(script.contains(r#"exec "$AZ_REAL" "$@""#));
    }

    #[test]
    fn reports_a_missing_azure_cli_rather_than_failing_obscurely() {
        let script = wrapper();
        assert!(script.contains("the Azure CLI is not installed"));
        assert!(script.contains("exit 127"));
    }

    #[test]
    fn is_a_standalone_sh_script_with_a_shebang() {
        // The wrapper is written to disk and invoked as `az`, so it must be a
        // self-contained script starting with `#!/bin/sh`. `render()` splits
        // the shebang off before the prelude and emits it first.
        let script = wrapper();
        assert!(
            script.starts_with("#!/bin/sh\n"),
            "the shebang must come first: {script}"
        );
    }
}
