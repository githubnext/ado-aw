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

    fn prepare_steps(&self, ctx: &CompileContext) -> Vec<String> {
        // Returns two YAML steps, in order:
        //
        // 1. Detection — runs in the Agent job's prepare phase (NOT a
        //    separate Setup job) so it shares the same pipeline-variable
        //    scope as the later AWF bash step. Sets `AW_AZ_MOUNTS` to
        //    either the two `--mount` args or empty string, depending
        //    on whether the host has azure-cli installed.
        //
        // 2. Conditional prompt append — appends an "Azure CLI" section
        //    to `/tmp/awf-tools/agent-prompt.md` so the agent knows
        //    `az` is on PATH inside the sandbox, what it's good for,
        //    and the auth model. Gated by
        //    `condition: ne(variables['AW_AZ_MOUNTS'], '')` so the
        //    agent only sees the advisory on runners where az was
        //    actually detected. The detection step above is the source
        //    of truth for that variable and MUST run first.
        //
        //    The advisory only advertises that `az devops` is
        //    auto-authenticated when `permissions.read` is configured —
        //    that subcommand reads `$AZURE_DEVOPS_EXT_PAT`, which is only
        //    populated when a read-only token (SC_READ_TOKEN) is minted
        //    from `permissions.read`. Advertising it otherwise would tell
        //    the agent to use a command that fails to authenticate.
        //
        // We do not implement `prompt_supplement()` because the
        // existing `wrap_prompt_append` helper doesn't emit a
        // `condition:` field. Emitting our own step here keeps the
        // trait API unchanged and confines the conditionality entirely
        // to this extension.
        let has_read = ctx
            .front_matter
            .permissions
            .as_ref()
            .and_then(|p| p.read.as_deref())
            .is_some_and(|s| !s.trim().is_empty());
        vec![self.detection_step(), self.prompt_append_step(has_read)]
    }
}

impl AzureCliExtension {
    /// Bash step that detects azure-cli on the host and sets the
    /// `AW_AZ_MOUNTS` pipeline variable. Always runs.
    ///
    /// Detection checks BOTH `/usr/bin/az` (the launcher shim) and
    /// `/opt/az` (the Python venv that az actually runs in). Mounting
    /// only one of the two would leave az partially available and
    /// produce confusing errors inside the sandbox.
    ///
    /// The setvariable value uses spaces between args so bash
    /// word-splits the unquoted `$(AW_AZ_MOUNTS)` expansion in the
    /// AWF invocation into clean `--mount <spec>` tokens. The value
    /// contains only path chars, `:`, and spaces — no shell
    /// metachars — so unquoted expansion is safe.
    ///
    /// Both branches MUST set the variable (the else branch sets it
    /// to empty string). If left undefined, ADO leaves the literal
    /// `$(AW_AZ_MOUNTS)` in subsequent bash steps, where bash
    /// interprets it as a `$(...)` command substitution, tries to
    /// run a program named `AW_AZ_MOUNTS`, gets exit 127, and the
    /// AWF invocation step dies under `set -e` — the opposite of
    /// graceful degradation. Defining the variable as empty makes
    /// ADO expand it to nothing, leaving a harmless `\`-continuation.
    fn detection_step(&self) -> String {
        r###"- bash: |
    set -eo pipefail
    if [ -f /usr/bin/az ] && [ -d /opt/az ]; then
      echo "##vso[task.setvariable variable=AW_AZ_MOUNTS]--mount /opt/az:/opt/az:ro --mount /usr/bin/az:/usr/bin/az:ro"
      echo "Azure CLI detected on host; mounting /opt/az and /usr/bin/az into AWF sandbox."
    else
      echo "##vso[task.setvariable variable=AW_AZ_MOUNTS]"
      echo "##vso[task.logissue type=warning]Azure CLI not detected on this runner (missing /usr/bin/az or /opt/az). The az command will not be available inside the agent sandbox. Install azure-cli on the runner image to enable it."
    fi
  displayName: "Detect Azure CLI on host (for AWF mount)"
"###
        .to_string()
    }

    /// Conditional `cat >>` step that appends an Azure CLI advisory
    /// section to the agent prompt file at pipeline time, only when
    /// the detection step above set `AW_AZ_MOUNTS` to non-empty.
    ///
    /// Uses a SINGLE-QUOTED heredoc delimiter (`<< 'AZURE_CLI_PROMPT_EOF'`)
    /// so `$AZURE_DEVOPS_EXT_PAT` and any other dollar references inside
    /// the prompt body are appended literally rather than expanded by
    /// bash. The closing delimiter is indented to match the bash block
    /// scalar style used by `wrap_prompt_append`.
    ///
    /// The `condition:` clause uses an ADO runtime expression. ADO
    /// evaluates it at step start against the variables visible at
    /// that moment — the detection step above has already run by
    /// then (steps execute sequentially within a job), so the
    /// expression sees the value just written by `setvariable`.
    ///
    /// displayName must stay in sync with the entry in
    /// `tests/bash_lint_tests.rs::REQUIRED_STEP_DISPLAY_NAMES`.
    ///
    /// `has_read` controls how the `az devops` bullet is phrased. That
    /// subcommand only authenticates automatically when `permissions.read`
    /// is configured (it reads `$AZURE_DEVOPS_EXT_PAT`, populated from the
    /// read-only token). When `read` is absent the bullet instead tells the
    /// agent the subcommand is unauthenticated, so it doesn't burn turns on
    /// `az devops` calls that will fail.
    fn prompt_append_step(&self, has_read: bool) -> String {
        // Shared prefix + subcommand list, kept in one place so the two
        // auth-state variants can never drift apart.
        const ADO_BULLET_PREFIX: &str =
            "- **Azure DevOps management** — `az devops`, `az pipelines`, `az repos`, `az boards`.";
        let ado_auth = if has_read {
            "These are authenticated automatically from `$AZURE_DEVOPS_EXT_PAT` (minted from `permissions: read:`). List/inspect operations Just Work; write operations honour the token's scopes."
        } else {
            "These are NOT authenticated: this pipeline declares no `permissions: read:`, so `$AZURE_DEVOPS_EXT_PAT` is unset and these commands will fail to authenticate. Ask the operator to add `permissions: read: <arm-service-connection>` to enable them."
        };
        let ado_bullet = format!("{ADO_BULLET_PREFIX} {ado_auth}");
        format!(
            r#"- bash: |
    cat >> "/tmp/awf-tools/agent-prompt.md" << 'AZURE_CLI_PROMPT_EOF'

    ---

    ## Azure CLI (`az`)

    The Azure CLI is available inside this sandbox at `/usr/bin/az`. Prefer it over hand-rolled curl calls when it covers what you need:

    {ado_bullet}
    - **Azure Resource Manager** — `az resource`, `az account`, `az group`. These require a separate Azure identity that ado-aw does not provision out of the box; sign in with `az login` using credentials supplied by another mechanism (e.g. a service connection writing them into your sandbox env) before invoking them.
    - **Microsoft Graph** — `az ad`, `az rest`. Same caveat as ARM.

    If a command you need isn't covered above, file a `missing-tool` safe output naming `azure-cli` so the operator can extend coverage rather than blocking on it silently.
    AZURE_CLI_PROMPT_EOF

    echo "Azure CLI prompt appended"
  displayName: "Append Azure CLI prompt"
  condition: ne(variables['AW_AZ_MOUNTS'], '')
"#
        )
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
        // Two prepare steps: [0] detection (always runs), [1] conditional
        // prompt-append (skipped when AW_AZ_MOUNTS is empty). The
        // detection step MUST stay at index 0 — it is what sets the
        // pipeline variable that the prompt-append step's
        // `condition:` reads.
        assert_eq!(
            steps.len(),
            2,
            "expected two prepare steps (detection, conditional prompt-append), got: {steps:?}"
        );
        let step = &steps[0];
        // Detection must check both the launcher shim and the venv
        // directory — mounting only one would leave az partially
        // available and produce confusing errors inside the sandbox.
        assert!(
            step.contains("[ -f /usr/bin/az ]"),
            "first prepare step (detection) must test for /usr/bin/az launcher: {step}"
        );
        assert!(
            step.contains("[ -d /opt/az ]"),
            "first prepare step (detection) must test for /opt/az venv directory: {step}"
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

    // ── Conditional prompt-append step (step index 1) ──────────────────────

    #[test]
    fn test_azure_cli_prompt_append_step_is_conditional() {
        // The prompt-append step MUST be gated by the AW_AZ_MOUNTS
        // pipeline variable so the agent only sees Azure CLI guidance
        // on runners where az was actually detected. Without this
        // gate the agent on a runner without az would be told to use
        // `az devops ...` and then fail with "command not found".
        let ext = AzureCliExtension;
        let fm = fm();
        let ctx = CompileContext::for_test(&fm);
        let steps = ext.prepare_steps(&ctx);
        let append = &steps[1];
        assert!(
            append.contains("condition: ne(variables['AW_AZ_MOUNTS'], '')"),
            "prompt-append step must be gated by condition: \
             ne(variables['AW_AZ_MOUNTS'], '') so it is skipped when \
             az is not detected on the host. Step:\n{append}"
        );
    }

    #[test]
    fn test_azure_cli_prompt_append_step_targets_agent_prompt_file() {
        // Must `cat >>` to the same path other extensions' supplements
        // use (the conventional `wrap_prompt_append` target) so the
        // agent reads everything from one file.
        let ext = AzureCliExtension;
        let fm = fm();
        let ctx = CompileContext::for_test(&fm);
        let append = &ext.prepare_steps(&ctx)[1];
        assert!(
            append.contains(r#"cat >> "/tmp/awf-tools/agent-prompt.md""#),
            "prompt-append step must append to /tmp/awf-tools/agent-prompt.md \
             (matching wrap_prompt_append). Step:\n{append}"
        );
    }

    #[test]
    fn test_azure_cli_prompt_append_step_has_advisory_anchors() {
        // Lock the advisory wording to the load-bearing parts: tool
        // names, env var, and the missing-tool escape hatch. Style
        // changes elsewhere in the prose are free; these anchors aren't.
        let ext = AzureCliExtension;
        let fm = fm();
        let ctx = CompileContext::for_test(&fm);
        let append = &ext.prepare_steps(&ctx)[1];
        for anchor in [
            "Azure CLI",
            "/usr/bin/az",
            "az devops",
            "AZURE_DEVOPS_EXT_PAT",
            "missing-tool",
        ] {
            assert!(
                append.contains(anchor),
                "prompt-append step must contain anchor `{anchor}`. Step:\n{append}"
            );
        }
    }

    #[test]
    fn test_azure_cli_prompt_append_uses_single_quoted_heredoc() {
        // The advisory body contains `$AZURE_DEVOPS_EXT_PAT` and other
        // literal dollar references. Single-quoting the heredoc
        // delimiter (`<< 'DELIM'`) is what prevents bash from
        // expanding them while building the prompt file. If anyone
        // ever swaps to an unquoted heredoc, `$AZURE_DEVOPS_EXT_PAT`
        // would be replaced by the runner's PAT value (a secret) and
        // baked into the agent prompt — a real leak.
        let ext = AzureCliExtension;
        let fm = fm();
        let ctx = CompileContext::for_test(&fm);
        let append = &ext.prepare_steps(&ctx)[1];
        assert!(
            append.contains("<< 'AZURE_CLI_PROMPT_EOF'"),
            "prompt-append heredoc delimiter must be single-quoted to \
             prevent expansion of $AZURE_DEVOPS_EXT_PAT and similar \
             literals inside the prompt body. Step:\n{append}"
        );
    }

    #[test]
    fn test_azure_cli_prompt_append_displayname_matches_lint_list() {
        // The lint test in tests/bash_lint_tests.rs has a coverage
        // list (REQUIRED_STEP_DISPLAY_NAMES) keyed on displayName.
        // If we ever rename this step the lint stops exercising it
        // silently. Lockstep the exact string here so a future rename
        // forces an explicit update in both places.
        let ext = AzureCliExtension;
        let fm = fm();
        let ctx = CompileContext::for_test(&fm);
        let append = &ext.prepare_steps(&ctx)[1];
        assert!(
            append.contains(r#"displayName: "Append Azure CLI prompt""#),
            "prompt-append step displayName must be exactly \
             \"Append Azure CLI prompt\" to match the coverage entry \
             in tests/bash_lint_tests.rs::REQUIRED_STEP_DISPLAY_NAMES. \
             Step:\n{append}"
        );
    }

    #[test]
    fn test_azure_cli_advisory_advertises_az_devops_auth_with_permissions_read() {
        // When permissions.read is configured the az devops bullet must
        // advertise it as auto-authenticated (no warning language).
        let ext = AzureCliExtension;
        let fm: FrontMatter = serde_yaml::from_str(
            "name: t\ndescription: x\npermissions:\n  read: my-read-sc\n",
        )
        .expect("front matter parses");
        let ctx = CompileContext::for_test(&fm);
        let append = &ext.prepare_steps(&ctx)[1];
        assert!(
            append.contains("authenticated automatically"),
            "with permissions.read, az devops bullet must advertise auto-auth. Step:\n{append}"
        );
        assert!(
            !append.contains("NOT authenticated"),
            "with permissions.read, the unauthenticated warning must not appear. Step:\n{append}"
        );
    }

    #[test]
    fn test_azure_cli_advisory_warns_az_devops_unauth_without_permissions_read() {
        // Without permissions.read the az devops bullet must warn that the
        // subcommand is unauthenticated rather than telling the agent it
        // Just Works.
        let ext = AzureCliExtension;
        let fm = fm(); // no permissions
        let ctx = CompileContext::for_test(&fm);
        let append = &ext.prepare_steps(&ctx)[1];
        assert!(
            append.contains("NOT authenticated"),
            "without permissions.read, az devops bullet must warn it is unauthenticated. Step:\n{append}"
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
