use super::{AwfMount, CompilerExtension, CompileContext, ExtensionPhase};

// ─── Azure CLI (always-on, install-free, gh-aw parity) ────────────────

/// Azure CLI extension.
///
/// Always-on internal extension that exposes the host's pre-installed
/// `az` binary to the agent inside the AWF Docker container (when
/// present), and adds the necessary Azure auth/management hosts to the
/// AWF allowlist so `az` calls aren't blocked by the L7 proxy.
///
/// **Install posture.** Mirrors gh-aw's "assume the CLI is on the
/// runner" model: this extension does NOT install `az`. Microsoft-hosted
/// `ubuntu-latest` agents ship with azure-cli pre-installed at
/// `/opt/az/` + `/usr/bin/az`. 1ES self-hosted pool operators are
/// responsible for baking `az` into their images if they want it
/// available to agents.
///
/// **Graceful runtime detection.** Instead of declaring static AWF
/// mounts (which would crash `docker run` with "bind source path does
/// not exist" on runners without azure-cli), this extension contributes
/// a [`prepare_steps`] bash step that runs in the Agent job *before*
/// the AWF invocation:
///
/// * If both `/usr/bin/az` and `/opt/az` exist on the host, the step
///   sets the ADO pipeline variable `AW_AZ_MOUNTS` to
///   `--mount /opt/az:/opt/az:ro --mount /usr/bin/az:/usr/bin/az:ro`
///   via `##vso[task.setvariable]`.
/// * Otherwise, the step sets `AW_AZ_MOUNTS` to the **empty string**
///   (still via `##vso[task.setvariable]`) and emits a
///   `##vso[task.logissue type=warning]` explaining `az` won't be
///   available inside the agent sandbox. Setting the variable to empty
///   is important: ADO leaves an *undefined* `$(VAR)` as the literal
///   string `$(VAR)` in later bash steps, where bash would interpret
///   it as a command substitution (`$(...)`) and fail under
///   `set -e` with exit 127. An empty-but-defined variable expands to
///   nothing, and the `$(AW_AZ_MOUNTS) \` line in the AWF chain
///   becomes a harmless `\`-continuation no-op.
///
/// The AWF invocation in `base.yml`/`1es-base.yml`/etc. then includes a
/// `$(AW_AZ_MOUNTS) \` line (injected by
/// [`crate::compile::common::generate_awf_mounts`] when `AzureCli` is
/// present in the extension list). At pipeline time this expands to
/// either the two `--mount` args or nothing — bash word-splits on the
/// expansion either way.
///
/// **Allowlist + bash command.** The 5 Azure auth/management hosts and
/// the `az` bash command name are added unconditionally — they are
/// inert when the runtime detection skips the mount (allowing hosts you
/// can't reach and a command that doesn't resolve is harmless and
/// keeps the compiled YAML deterministic across runner types).
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
        // Intentionally empty — declaring static mounts here would cause
        // `docker run` to fail with "bind source path does not exist" on
        // runners that don't have azure-cli pre-installed (e.g. some 1ES
        // self-hosted pools). The mounts are decided at pipeline time
        // by `prepare_steps` below, which sets the `AW_AZ_MOUNTS`
        // pipeline variable; `generate_awf_mounts` then injects a
        // `$(AW_AZ_MOUNTS) \` line into the AWF invocation that expands
        // to the mounts when az is present and to nothing when it isn't.
        vec![]
    }

    fn prepare_steps(&self, _ctx: &CompileContext) -> Vec<String> {
        // Runtime detection step. Runs in the Agent job's prepare phase
        // (NOT a separate Setup job) so it shares the same pipeline-
        // variable scope as the subsequent AWF bash step. ADO pipeline
        // variables set via `##vso[task.setvariable]` are visible as
        // `$(NAME)` in later steps of the same job.
        //
        // Detection checks both /usr/bin/az (the launcher shim) AND
        // /opt/az (the Python venv that az actually runs in). Mounting
        // only one of the two would leave az partially available and
        // produce confusing errors inside the sandbox.
        //
        // The setvariable value uses spaces between args so bash
        // word-splits the unquoted `$(AW_AZ_MOUNTS)` expansion in the
        // AWF invocation into clean `--mount <spec>` tokens. The value
        // contains only path chars, `:`, and spaces — no shell
        // metachars — so unquoted expansion is safe.
        //
        // Both branches MUST set the variable (the else branch sets it
        // to empty string). If left undefined, ADO leaves the literal
        // `$(AW_AZ_MOUNTS)` in subsequent bash steps, where bash
        // interprets it as a `$(...)` command substitution, tries to
        // run a program named `AW_AZ_MOUNTS`, gets exit 127, and the
        // AWF invocation step dies under `set -e` — the opposite of
        // graceful degradation. Defining the variable as empty makes
        // ADO expand it to nothing, leaving a harmless `\`-continuation.
        //
        // Warning text is intentionally short and operator-facing.
        // Agents that don't invoke `az` are unaffected; agents that do
        // will get a normal "command not found" inside the sandbox.
        let step = r###"- bash: |
    set -eo pipefail
    if [ -f /usr/bin/az ] && [ -d /opt/az ]; then
      echo "##vso[task.setvariable variable=AW_AZ_MOUNTS]--mount /opt/az:/opt/az:ro --mount /usr/bin/az:/usr/bin/az:ro"
      echo "Azure CLI detected on host; mounting /opt/az and /usr/bin/az into AWF sandbox."
    else
      echo "##vso[task.setvariable variable=AW_AZ_MOUNTS]"
      echo "##vso[task.logissue type=warning]Azure CLI not detected on this runner (missing /usr/bin/az or /opt/az). The az command will not be available inside the agent sandbox. Install azure-cli on the runner image to enable it."
    fi
  displayName: "Detect Azure CLI on host (for AWF mount)"
"###;
        vec![step.to_string()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::extensions::CompileContext;
    use crate::compile::types::FrontMatter;

    fn fm() -> FrontMatter {
        serde_yaml::from_str("name: t\ndescription: x\n").expect("front matter parses")
    }

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
    fn test_azure_cli_required_awf_mounts_is_empty_static() {
        // The static mount list must stay empty so `docker run` does not
        // fail with "bind source path does not exist" on runners without
        // azure-cli. Mounts are contributed via the pipeline variable
        // `AW_AZ_MOUNTS` set by `prepare_steps` below and injected into
        // the AWF chain by `generate_awf_mounts`.
        let ext = AzureCliExtension;
        assert!(
            ext.required_awf_mounts().is_empty(),
            "AzureCli must not contribute STATIC AWF mounts — the runner may not have az installed"
        );
    }

    #[test]
    fn test_azure_cli_prepare_steps_detects_az_before_setting_var() {
        let ext = AzureCliExtension;
        let fm = fm();
        let ctx = CompileContext::for_test(&fm);
        let steps = ext.prepare_steps(&ctx);
        assert_eq!(
            steps.len(),
            1,
            "expected exactly one prepare step (the az detection step), got: {steps:?}"
        );
        let step = &steps[0];
        // Detection must check both the launcher shim and the venv
        // directory — mounting only one would leave az partially
        // available and produce confusing errors inside the sandbox.
        assert!(
            step.contains("[ -f /usr/bin/az ]"),
            "prepare step must test for /usr/bin/az launcher: {step}"
        );
        assert!(
            step.contains("[ -d /opt/az ]"),
            "prepare step must test for /opt/az venv directory: {step}"
        );
    }

    #[test]
    fn test_azure_cli_prepare_steps_sets_aw_az_mounts_pipeline_var() {
        let ext = AzureCliExtension;
        let fm = fm();
        let ctx = CompileContext::for_test(&fm);
        let step = ext.prepare_steps(&ctx).into_iter().next().unwrap();
        // Must use ##vso[task.setvariable] to make the value visible as
        // $(AW_AZ_MOUNTS) in the subsequent AWF bash step.
        assert!(
            step.contains("##vso[task.setvariable variable=AW_AZ_MOUNTS]"),
            "must set AW_AZ_MOUNTS pipeline variable: {step}"
        );
        // The value must contain both --mount args so the AWF
        // invocation gets both /opt/az and /usr/bin/az.
        assert!(
            step.contains("--mount /opt/az:/opt/az:ro"),
            "must include /opt/az mount in the setvariable value: {step}"
        );
        assert!(
            step.contains("--mount /usr/bin/az:/usr/bin/az:ro"),
            "must include /usr/bin/az mount in the setvariable value: {step}"
        );
    }

    #[test]
    fn test_azure_cli_prepare_steps_warns_when_az_missing() {
        let ext = AzureCliExtension;
        let fm = fm();
        let ctx = CompileContext::for_test(&fm);
        let step = ext.prepare_steps(&ctx).into_iter().next().unwrap();
        // Must surface a visible ADO warning so operators can see why
        // `az` isn't available inside their sandbox instead of silently
        // failing later with "command not found".
        assert!(
            step.contains("##vso[task.logissue type=warning]"),
            "must emit an ADO warning when az is not detected: {step}"
        );
        assert!(
            step.contains("Azure CLI not detected"),
            "warning text must explain the cause: {step}"
        );
        // The `else` branch of the `if` must be the warning branch — so
        // the warning is the missing-az path, not the detected-az path.
        assert!(
            step.contains("else") && step.contains("fi"),
            "must use a proper if/else/fi structure: {step}"
        );
    }

    #[test]
    fn test_azure_cli_prepare_steps_defines_aw_az_mounts_in_else_branch() {
        // Regression guard for the graceful-degradation bug:
        // if the `else` branch doesn't explicitly setvariable on
        // AW_AZ_MOUNTS, ADO leaves the literal `$(AW_AZ_MOUNTS)` in
        // the subsequent AWF bash step, bash interprets it as a
        // `$(...)` command substitution, tries to execute a program
        // named AW_AZ_MOUNTS, gets exit 127, and `set -e` kills the
        // step — exactly the failure mode this PR set out to prevent.
        let ext = AzureCliExtension;
        let fm = fm();
        let ctx = CompileContext::for_test(&fm);
        let step = ext.prepare_steps(&ctx).into_iter().next().unwrap();

        // Count setvariable occurrences — must be 2 (one per branch).
        let setvar_count = step
            .matches("##vso[task.setvariable variable=AW_AZ_MOUNTS]")
            .count();
        assert_eq!(
            setvar_count, 2,
            "AW_AZ_MOUNTS must be set in BOTH branches of the if/else (got {setvar_count}); \
             leaving it undefined in the missing-az branch causes bash to interpret \
             the literal `$(AW_AZ_MOUNTS)` as command substitution and fail under set -e. \
             Step:\n{step}"
        );

        // Verify the else branch sets it to empty (no `--mount` chars
        // after the `]`). We slice the step from "else" to "fi" and
        // assert the else block contains a setvariable line that ends
        // with `]"` (closing-bracket-then-quote = empty value).
        let else_start = step.find("else").expect("must have else branch");
        let fi_end = step[else_start..].find("fi").expect("must have fi");
        let else_block = &step[else_start..else_start + fi_end];
        assert!(
            else_block.contains("##vso[task.setvariable variable=AW_AZ_MOUNTS]\""),
            "else branch must set AW_AZ_MOUNTS to empty string (line must end with `]\"`), got:\n{else_block}"
        );
        // And the else branch must NOT include any --mount arg (would
        // mean we're accidentally setting non-empty when az is missing).
        assert!(
            !else_block.contains("--mount"),
            "else branch must not contain --mount args (those belong to the detected branch only): {else_block}"
        );
    }

    #[test]
    fn test_azure_cli_prepare_steps_uses_pipefail() {
        // Bash steps in this repo's lint policy require `set -eo
        // pipefail` to avoid silent failure of any intermediate command.
        let ext = AzureCliExtension;
        let fm = fm();
        let ctx = CompileContext::for_test(&fm);
        let step = ext.prepare_steps(&ctx).into_iter().next().unwrap();
        assert!(
            step.contains("set -eo pipefail"),
            "detection bash step must use set -eo pipefail: {step}"
        );
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
    fn test_azure_cli_no_path_prepends() {
        // Sanity check that the install-free posture isn't accidentally
        // regressed by a future edit that adds a PATH munge.
        let ext = AzureCliExtension;
        assert!(
            ext.awf_path_prepends().is_empty(),
            "must not prepend any PATH entry — /usr/bin is already on PATH inside AWF"
        );
    }
}
