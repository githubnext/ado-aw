use super::{CompileContext, CompilerExtension, Declarations, ExtensionPhase};
use crate::compile::ir::condition::{Condition, Expr};
use crate::compile::ir::step::{BashStep, Step};

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
/// a typed Agent-job prepare bash step that runs *before*
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

    /// The two Agent-job prepare steps. The
    /// detection step exports `AW_AZ_MOUNTS` via
    /// `##vso[task.setvariable]` (a *pipeline variable*, not a step
    /// output, so it's referenced via `variables['AW_AZ_MOUNTS']`,
    /// not `$(detect.AW_AZ_MOUNTS)`). The conditional prompt-append
    /// step uses [`Condition::Ne`] of that pipeline variable against
    /// the empty-string literal — same wire shape as today's
    /// `condition: ne(variables['AW_AZ_MOUNTS'], '')`.
    fn declarations(&self, _ctx: &CompileContext) -> anyhow::Result<Declarations> {
        Ok(Declarations {
            network_hosts: vec![
                // OAuth + sign-in
                "login.microsoftonline.com".to_string(),
                "login.windows.net".to_string(),
                // ARM (resource management)
                "management.azure.com".to_string(),
                // Microsoft Graph
                "graph.microsoft.com".to_string(),
                // Microsoft's link shortener used by az subcommand help / metadata
                "aka.ms".to_string(),
            ],
            bash_commands: vec!["az".to_string()],
            agent_prepare_steps: vec![
                Step::Bash(detection_bash_step()),
                Step::Bash(prompt_append_bash_step()),
            ],
            ..Declarations::default()
        })
    }
}

/// Detect azure-cli on the host and set the `AW_AZ_MOUNTS` pipeline
/// variable for the later AWF invocation.
fn detection_bash_step() -> BashStep {
    let script = "set -eo pipefail\n\
        if [ -f /usr/bin/az ] && [ -d /opt/az ]; then\n  \
          echo \"##vso[task.setvariable variable=AW_AZ_MOUNTS]--mount /opt/az:/opt/az:ro --mount /usr/bin/az:/usr/bin/az:ro\"\n  \
          echo \"Azure CLI detected on host; mounting /opt/az and /usr/bin/az into AWF sandbox.\"\n\
        else\n  \
          echo \"##vso[task.setvariable variable=AW_AZ_MOUNTS]\"\n  \
          echo \"##vso[task.logissue type=warning]Azure CLI not detected on this runner (missing /usr/bin/az or /opt/az). The az command will not be available inside the agent sandbox. Install azure-cli on the runner image to enable it.\"\n\
        fi\n";
    BashStep::new("Detect Azure CLI on host (for AWF mount)", script)
}

/// Append an Azure CLI advisory when the detection step found `az`.
fn prompt_append_bash_step() -> BashStep {
    let script = "cat >> \"/tmp/awf-tools/agent-prompt.md\" << 'AZURE_CLI_PROMPT_EOF'\n\
\n\
---\n\
\n\
## Azure CLI (`az`)\n\
\n\
The Azure CLI is available inside this sandbox at `/usr/bin/az`. Prefer it over hand-rolled curl calls when it covers what you need:\n\
\n\
- **Azure DevOps management** \u{2014} `az devops`, `az pipelines`, `az repos`, `az boards`. These are authenticated automatically from `$AZURE_DEVOPS_EXT_PAT` when the pipeline declares `permissions: read:`. List/inspect operations Just Work; write operations honour the PAT's scopes.\n\
- **Azure Resource Manager** \u{2014} `az resource`, `az account`, `az group`. These require a separate Azure identity that ado-aw does not provision out of the box; sign in with `az login` using credentials supplied by another mechanism (e.g. a service connection writing them into your sandbox env) before invoking them.\n\
- **Microsoft Graph** \u{2014} `az ad`, `az rest`. Same caveat as ARM.\n\
\n\
If a command you need isn't covered above, file a `missing-tool` safe output naming `azure-cli` so the operator can extend coverage rather than blocking on it silently.\n\
AZURE_CLI_PROMPT_EOF\n\
\n\
echo \"Azure CLI prompt appended\"\n";
    BashStep::new("Append Azure CLI prompt", script).with_condition(Condition::Ne(
        Expr::Variable("AW_AZ_MOUNTS".to_string()),
        Expr::Literal(String::new()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::extensions::CompileContext;
    use crate::compile::types::FrontMatter;

    fn fm() -> FrontMatter {
        serde_yaml::from_str("name: t\ndescription: x\n").expect("front matter parses")
    }

    fn agent_prepare_steps(ext: &AzureCliExtension, ctx: &CompileContext<'_>) -> Vec<Step> {
        ext.declarations(ctx).unwrap().agent_prepare_steps
    }

    fn bash_step(step: &Step) -> &BashStep {
        match step {
            Step::Bash(b) => b,
            other => panic!("expected Step::Bash, got {other:?}"),
        }
    }

    #[test]
    fn test_azure_cli_required_hosts_includes_login_microsoft() {
        let ext = AzureCliExtension;
        let fm = fm();
        let ctx = CompileContext::for_test(&fm);
        let hosts = ext.declarations(&ctx).unwrap().network_hosts;
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
        // `AW_AZ_MOUNTS` set by the typed prepare declaration and injected into
        // the AWF chain by `generate_awf_mounts`.
        let ext = AzureCliExtension;
        let fm = fm();
        let ctx = CompileContext::for_test(&fm);
        assert!(
            ext.declarations(&ctx).unwrap().awf_mounts.is_empty(),
            "AzureCli must not contribute STATIC AWF mounts — the runner may not have az installed"
        );
    }

    #[test]
    fn test_azure_cli_declarations_detects_az_before_setting_var() {
        let ext = AzureCliExtension;
        let fm = fm();
        let ctx = CompileContext::for_test(&fm);
        let steps = agent_prepare_steps(&ext, &ctx);
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
        let step = bash_step(&steps[0]);
        // Detection must check both the launcher shim and the venv
        // directory — mounting only one would leave az partially
        // available and produce confusing errors inside the sandbox.
        assert!(
            step.script.contains("[ -f /usr/bin/az ]"),
            "first prepare step (detection) must test for /usr/bin/az launcher: {}",
            step.script
        );
        assert!(
            step.script.contains("[ -d /opt/az ]"),
            "first prepare step (detection) must test for /opt/az venv directory: {}",
            step.script
        );
    }

    #[test]
    fn test_azure_cli_declarations_sets_aw_az_mounts_pipeline_var() {
        let ext = AzureCliExtension;
        let fm = fm();
        let ctx = CompileContext::for_test(&fm);
        let steps = agent_prepare_steps(&ext, &ctx);
        let step = bash_step(&steps[0]);
        // Must use ##vso[task.setvariable] to make the value visible as
        // $(AW_AZ_MOUNTS) in the subsequent AWF bash step.
        assert!(
            step.script
                .contains("##vso[task.setvariable variable=AW_AZ_MOUNTS]"),
            "must set AW_AZ_MOUNTS pipeline variable: {}",
            step.script
        );
        // The value must contain both --mount args so the AWF
        // invocation gets both /opt/az and /usr/bin/az.
        assert!(
            step.script.contains("--mount /opt/az:/opt/az:ro"),
            "must include /opt/az mount in the setvariable value: {}",
            step.script
        );
        assert!(
            step.script.contains("--mount /usr/bin/az:/usr/bin/az:ro"),
            "must include /usr/bin/az mount in the setvariable value: {}",
            step.script
        );
    }

    #[test]
    fn test_azure_cli_declarations_warns_when_az_missing() {
        let ext = AzureCliExtension;
        let fm = fm();
        let ctx = CompileContext::for_test(&fm);
        let steps = agent_prepare_steps(&ext, &ctx);
        let step = bash_step(&steps[0]);
        // Must surface a visible ADO warning so operators can see why
        // `az` isn't available inside their sandbox instead of silently
        // failing later with "command not found".
        assert!(
            step.script.contains("##vso[task.logissue type=warning]"),
            "must emit an ADO warning when az is not detected: {}",
            step.script
        );
        assert!(
            step.script.contains("Azure CLI not detected"),
            "warning text must explain the cause: {}",
            step.script
        );
        // The `else` branch of the `if` must be the warning branch — so
        // the warning is the missing-az path, not the detected-az path.
        assert!(
            step.script.contains("else") && step.script.contains("fi"),
            "must use a proper if/else/fi structure: {}",
            step.script
        );
    }

    #[test]
    fn test_azure_cli_declarations_defines_aw_az_mounts_in_else_branch() {
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
        let steps = agent_prepare_steps(&ext, &ctx);
        let step = bash_step(&steps[0]);

        // Count setvariable occurrences — must be 2 (one per branch).
        let setvar_count = step
            .script
            .matches("##vso[task.setvariable variable=AW_AZ_MOUNTS]")
            .count();
        assert_eq!(
            setvar_count, 2,
            "AW_AZ_MOUNTS must be set in BOTH branches of the if/else (got {setvar_count}); \
             leaving it undefined in the missing-az branch causes bash to interpret \
             the literal `$(AW_AZ_MOUNTS)` as command substitution and fail under set -e. \
             Step:\n{}",
            step.script
        );

        // Verify the else branch sets it to empty (no `--mount` chars
        // after the `]`). We slice the step from "else" to "fi" and
        // assert the else block contains a setvariable line that ends
        // with `]"` (closing-bracket-then-quote = empty value).
        let else_start = step.script.find("else").expect("must have else branch");
        let fi_end = step.script[else_start..].find("fi").expect("must have fi");
        let else_block = &step.script[else_start..else_start + fi_end];
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
    fn test_azure_cli_declarations_uses_pipefail() {
        // Bash steps in this repo's lint policy require `set -eo
        // pipefail` to avoid silent failure of any intermediate command.
        let ext = AzureCliExtension;
        let fm = fm();
        let ctx = CompileContext::for_test(&fm);
        let steps = agent_prepare_steps(&ext, &ctx);
        let step = bash_step(&steps[0]);
        assert!(
            step.script.contains("set -eo pipefail"),
            "detection bash step must use set -eo pipefail: {}",
            step.script
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
        let steps = agent_prepare_steps(&ext, &ctx);
        let append = bash_step(&steps[1]);
        assert!(matches!(
            append.condition,
            Some(Condition::Ne(
                Expr::Variable(ref var),
                Expr::Literal(ref literal)
            )) if var == "AW_AZ_MOUNTS" && literal.is_empty()
        ));
    }

    #[test]
    fn test_azure_cli_prompt_append_step_targets_agent_prompt_file() {
        // Must `cat >>` to the same path other extensions' supplements
        // use (the conventional `wrap_prompt_append` target) so the
        // agent reads everything from one file.
        let ext = AzureCliExtension;
        let fm = fm();
        let ctx = CompileContext::for_test(&fm);
        let steps = agent_prepare_steps(&ext, &ctx);
        let append = bash_step(&steps[1]);
        assert!(
            append
                .script
                .contains(r#"cat >> "/tmp/awf-tools/agent-prompt.md""#),
            "prompt-append step must append to /tmp/awf-tools/agent-prompt.md \
             (matching wrap_prompt_append). Step:\n{}",
            append.script
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
        let steps = agent_prepare_steps(&ext, &ctx);
        let append = bash_step(&steps[1]);
        for anchor in [
            "Azure CLI",
            "/usr/bin/az",
            "az devops",
            "AZURE_DEVOPS_EXT_PAT",
            "missing-tool",
        ] {
            assert!(
                append.script.contains(anchor),
                "prompt-append step must contain anchor `{anchor}`. Step:\n{}",
                append.script
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
        let steps = agent_prepare_steps(&ext, &ctx);
        let append = bash_step(&steps[1]);
        assert!(
            append.script.contains("<< 'AZURE_CLI_PROMPT_EOF'"),
            "prompt-append heredoc delimiter must be single-quoted to \
             prevent expansion of $AZURE_DEVOPS_EXT_PAT and similar \
             literals inside the prompt body. Step:\n{}",
            append.script
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
        let steps = agent_prepare_steps(&ext, &ctx);
        let append = bash_step(&steps[1]);
        assert_eq!(append.display_name, "Append Azure CLI prompt");
    }

    #[test]
    fn test_azure_cli_required_bash_commands_includes_az() {
        let ext = AzureCliExtension;
        let fm = fm();
        let ctx = CompileContext::for_test(&fm);
        let cmds = ext.declarations(&ctx).unwrap().bash_commands;
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
        let fm = fm();
        let ctx = CompileContext::for_test(&fm);
        assert!(
            ext.declarations(&ctx).unwrap().awf_path_prepends.is_empty(),
            "must not prepend any PATH entry — /usr/bin is already on PATH inside AWF"
        );
    }
}
