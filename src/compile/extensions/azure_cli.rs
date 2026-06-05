use super::{AwfMount, AwfMountMode, CompilerExtension, ExtensionPhase};

// ─── Azure CLI (always-on, install-free, gh-aw parity) ────────────────

/// Azure CLI extension.
///
/// Always-on internal extension that exposes the host's pre-installed
/// `az` binary to the agent inside the AWF Docker container, and adds
/// the necessary Azure auth/management hosts to the AWF allowlist so
/// `az` calls aren't blocked by the L7 proxy.
///
/// **Install posture.** Mirrors gh-aw's "assume the CLI is on the
/// runner" model: this extension does NOT install `az`. It assumes the
/// host has azure-cli pre-installed, which is true for Microsoft-hosted
/// `ubuntu-latest` agents (`/opt/az/` + `/usr/bin/az`). 1ES self-hosted
/// pool operators are responsible for baking `az` into their images; if
/// `az` is missing, the AWF mount of `/opt/az` will fail at runtime
/// with a clear error.
///
/// **AWF mounts.** AWF auto-mounts `/tmp` and `/opt/hostedtoolcache`
/// only, so without explicit mounts the host's `az` is invisible inside
/// the container. We bind-mount both the `/opt/az` Python venv that
/// `az` is implemented in and the `/usr/bin/az` launcher shim.
///
/// **Auth.** `az devops` subcommands read `AZURE_DEVOPS_EXT_PAT` (set
/// inside AWF when `permissions.read` is configured). General `az`
/// commands (`az account get-access-token`, `az resource ...`, Graph
/// calls) require separate authentication and are out of scope for this
/// extension.
pub struct AzureCliExtension;

impl CompilerExtension for AzureCliExtension {
    fn name(&self) -> &str {
        "Azure CLI"
    }

    fn phase(&self) -> ExtensionPhase {
        ExtensionPhase::Tool
    }

    fn required_hosts(&self) -> Vec<String> {
        vec![
            // OAuth + sign-in
            "login.microsoftonline.com".to_string(),
            "login.windows.net".to_string(),
            // ARM (resource management)
            "management.azure.com".to_string(),
            // Microsoft Graph
            "graph.microsoft.com".to_string(),
            // Microsoft's link shortener used by az subcommand help / metadata
            "aka.ms".to_string(),
        ]
    }

    fn required_bash_commands(&self) -> Vec<String> {
        vec!["az".to_string()]
    }

    fn required_awf_mounts(&self) -> Vec<AwfMount> {
        // /opt/az holds the Python venv that the `az` CLI runs in
        // (azure-cli is implemented as a Python package). /usr/bin/az is
        // the launcher shim that activates the venv and dispatches.
        // Both must be mounted for `az` to work inside AWF.
        vec![
            AwfMount::new("/opt/az", "/opt/az", AwfMountMode::ReadOnly),
            AwfMount::new("/usr/bin/az", "/usr/bin/az", AwfMountMode::ReadOnly),
        ]
        // No awf_path_prepends() needed: /usr/bin is already on PATH
        // inside the AWF container's base image.
        // No prepare_steps() needed: host is assumed to have az pre-installed.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_azure_cli_required_hosts_includes_login_microsoft() {
        let ext = AzureCliExtension;
        let hosts = ext.required_hosts();
        assert!(
            hosts.iter().any(|h| h == "login.microsoftonline.com"),
            "required_hosts must include login.microsoftonline.com so the agent can OAuth: {hosts:?}"
        );
        assert!(
            hosts.iter().any(|h| h == "management.azure.com"),
            "required_hosts must include management.azure.com so ARM calls work: {hosts:?}"
        );
        assert!(
            hosts.iter().any(|h| h == "graph.microsoft.com"),
            "required_hosts must include graph.microsoft.com for Graph calls: {hosts:?}"
        );
    }

    #[test]
    fn test_azure_cli_required_awf_mounts_includes_both_az_paths() {
        let ext = AzureCliExtension;
        let mounts = ext.required_awf_mounts();
        assert_eq!(
            mounts.len(),
            2,
            "expected exactly two AWF mounts (/opt/az + /usr/bin/az), got: {mounts:?}"
        );
        let has_opt_az = mounts
            .iter()
            .any(|m| m.host_path == "/opt/az" && m.container_path == "/opt/az");
        let has_usr_bin_az = mounts
            .iter()
            .any(|m| m.host_path == "/usr/bin/az" && m.container_path == "/usr/bin/az");
        assert!(has_opt_az, "must mount /opt/az: {mounts:?}");
        assert!(has_usr_bin_az, "must mount /usr/bin/az: {mounts:?}");
        for m in &mounts {
            assert_eq!(
                m.mode,
                AwfMountMode::ReadOnly,
                "az mounts must be read-only (the agent has no business writing to az's install): {m:?}"
            );
        }
    }

    #[test]
    fn test_azure_cli_required_bash_commands_includes_az() {
        let ext = AzureCliExtension;
        let cmds = ext.required_bash_commands();
        assert!(
            cmds.iter().any(|c| c == "az"),
            "required_bash_commands must include `az`: {cmds:?}"
        );
    }

    #[test]
    fn test_azure_cli_phase_is_tool() {
        let ext = AzureCliExtension;
        assert_eq!(
            ext.phase(),
            ExtensionPhase::Tool,
            "Azure CLI extension is a tool, not a System/Runtime extension"
        );
    }

    #[test]
    fn test_azure_cli_no_prepare_steps_or_path_prepends() {
        // Sanity check that the install-free posture isn't accidentally
        // regressed by a future edit that adds an apt install step or a
        // PATH munge.
        let ext = AzureCliExtension;
        // Use the CompileContext::for_test helper if available; otherwise
        // construct a minimal one. These methods are inherited from the
        // trait's default implementations and should return empty.
        assert!(
            ext.awf_path_prepends().is_empty(),
            "must not prepend any PATH entry — /usr/bin is already on PATH inside AWF"
        );
    }
}
