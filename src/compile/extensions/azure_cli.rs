use super::{CompileContext, CompilerExtension, Declarations, ExtensionPhase};
use crate::ado_proxy::catalog::Capability;
use crate::compile::common::{
    ADO_MCP_TOKEN_SENTINEL, ADO_PROXY_CONTAINER_NAME, ADO_PROXY_LISTEN_PORT, AZ_WRAPPER_DIR,
    AZ_WRAPPER_PATH,
};
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
/// **Auth.** This extension only exposes the binary. It does not inject an
/// Azure or Azure DevOps credential into the agent sandbox.
/// `permissions.read` authenticates the optional first-party Azure DevOps MCP
/// backend; it does not populate `AZURE_DEVOPS_EXT_PAT` for direct CLI use.
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
    fn declarations(&self, ctx: &CompileContext) -> anyhow::Result<Declarations> {
        let proxied = crate::compile::common::ado_proxy_enabled(ctx.front_matter);
        let capabilities = crate::compile::common::ado_proxy_capabilities(ctx.front_matter);

        let mut agent_prepare_steps = vec![Step::Bash(detection_bash_step())];
        if proxied {
            // Installed before the prompt is appended so the advisory and the
            // wrapper cannot describe different worlds.
            agent_prepare_steps.push(Step::Bash(install_az_wrapper_step(&capabilities)));
            // This advisory is independent of `az` detection: the same policy
            // governs MCP reads, and the agent must understand effective
            // front-matter scope even on a runner without Azure CLI.
            agent_prepare_steps.push(Step::Bash(proxy_policy_prompt_step(
                ctx.front_matter,
                &capabilities,
            )));
        }
        agent_prepare_steps.push(Step::Bash(prompt_append_bash_step(proxied, &capabilities)));

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
            agent_prepare_steps,
            // Shadow the real `az` with the wrapper. Both the file and this
            // prepend are needed: the agent runs in a chroot, so the container's
            // /usr/local/bin is not the chroot's, and only PATH order decides
            // which binary the agent actually invokes. AWF installs its own `gh`
            // wrapper the same way.
            awf_path_prepends: if proxied {
                vec![AZ_WRAPPER_DIR.to_string()]
            } else {
                Vec::new()
            },
            ..Declarations::default()
        })
    }
}

/// Install the generated `az` wrapper into the sandbox.
///
/// No mount is required: AWF bind-mounts the runner's `/tmp` into the agent
/// chroot, which is the same mechanism that delivers the agent prompt and the
/// Copilot binary. Writing the file here therefore makes it visible to the
/// agent at the same path.
///
/// Gated on the same `AW_AZ_MOUNTS` signal as the prompt advisory: with no
/// `az` on the runner there is nothing for the wrapper to exec, and shadowing a
/// missing binary would turn a clear "command not found" into a confusing
/// wrapper error.
fn install_az_wrapper_step(capabilities: &[Capability]) -> BashStep {
    let wrapper = crate::compile::az_wrapper::render_az_wrapper(
        ADO_PROXY_CONTAINER_NAME,
        ADO_PROXY_LISTEN_PORT,
        ADO_MCP_TOKEN_SENTINEL,
        capabilities,
    );
    // Indent the body for the heredoc without altering its content.
    let script = format!(
        "set -eo pipefail\n\
         mkdir -p {AZ_WRAPPER_DIR}\n\
         cat > '{AZ_WRAPPER_PATH}' << 'ADO_AW_AZ_WRAPPER_EOF'\n\
         {wrapper}\n\
         ADO_AW_AZ_WRAPPER_EOF\n\
         chmod 755 '{AZ_WRAPPER_PATH}'\n\
         echo \"az wrapper installed at {AZ_WRAPPER_PATH}\"\n"
    );
    BashStep::new("Install az wrapper (ado-proxy)", script).with_condition(Condition::Ne(
        Expr::Variable("AW_AZ_MOUNTS".to_string()),
        Expr::Literal(String::new()),
    ))
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

/// Explain the effective compiler-owned ADO read policy to the agent.
///
/// This is generated from the same front matter as `PolicyDocument`, so prompt
/// guidance cannot claim a scope the runtime denies (or hide one it allows).
/// Runtime denial responses and the sanitized decision log remain
/// authoritative; this text prevents predictable prompt/config conflicts
/// before the agent starts retrying an impossible request.
fn proxy_policy_prompt_step(
    front_matter: &crate::compile::types::FrontMatter,
    capabilities: &[Capability],
) -> BashStep {
    let capability_list = capabilities
        .iter()
        .map(|capability| format!("`{}`", capability.as_str()))
        .collect::<Vec<_>>()
        .join(", ");

    let mut scope_lines = vec![
        "- Current organization, project, and repository (by name or GUID).".to_string(),
    ];
    if let Some(options) = front_matter
        .permissions
        .as_ref()
        .and_then(|permissions| permissions.read.as_ref())
        .and_then(crate::compile::types::ReadPermissionConfig::options)
    {
        for organization in &options.allow {
            for project in &organization.projects {
                let repositories = if project.repositories.is_empty() {
                    "project-scoped reads; no repository-scoped reads".to_string()
                } else {
                    format!(
                        "project-scoped reads; repositories: {}",
                        project
                            .repositories
                            .iter()
                            .map(|repository| format!("`{}`", repository.as_str()))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                scope_lines.push(format!(
                    "- Additional `{}/{}` ({repositories}).",
                    organization.organization.as_str(),
                    project.project.as_str(),
                ));
            }
        }
    }
    for repository in &front_matter.repositories {
        if repository.repo_type.eq_ignore_ascii_case("git")
            && let Some((project, name)) = repository.name.split_once('/')
        {
            scope_lines.push(format!(
                "- Repository-only `{project}/{name}` from `repos:`; this does **not** grant project work items, builds, or pipelines."
            ));
        }
    }
    let scope_list = scope_lines.join("\n");

    let body = format!(
        "\n\
---\n\
\n\
## Azure DevOps read policy\n\
\n\
Azure DevOps reads are routed through a credential-isolated policy proxy. You, `az`, and the Azure DevOps MCP have no real Azure DevOps credential.\n\
\n\
**Enabled capabilities:** {capability_list}\n\
\n\
**Allowed scopes:**\n\
{scope_list}\n\
\n\
Requests outside these capabilities or scopes, all writes, and secret-bearing route families are deliberately refused. A refusal is a policy result, not an authentication problem: do not sign in, change the URL, or retry it as a workaround. The error response names the denial reason, and sanitized proxy decision logs are published with the run for operators.\n\
\n\
If your task requires a read outside this list, report it as missing data/tooling and name the exact organization, project, repository, and operation that the front matter would need to grant.\n"
    );
    let script = format!(
        "cat >> \"/tmp/awf-tools/agent-prompt.md\" << 'ADO_PROXY_POLICY_PROMPT_EOF'\n\
{body}\
ADO_PROXY_POLICY_PROMPT_EOF\n\
\n\
echo \"ado-proxy policy prompt appended\"\n"
    );
    BashStep::new("Append ado-proxy policy prompt", script)
}

/// Append an Azure CLI advisory when the detection step found `az`.
///
/// Two quite different messages, because the agent's actual capability differs.
/// Getting this wrong is not cosmetic: an agent told a command is unavailable
/// will not try it, and one told it has access it lacks will retry a failing
/// call or invent a workaround. The unproxied text deliberately claims nothing
/// beyond "not pre-authenticated" — an earlier revision overclaimed here.
fn prompt_append_bash_step(proxied: bool, capabilities: &[Capability]) -> BashStep {
    let body = if proxied {
        let groups = crate::compile::common::az_allowed_groups(capabilities);
        let group_list = groups
            .iter()
            .map(|g| format!("`az {g}`"))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "\n\
---\n\
\n\
## Azure CLI (`az`)\n\
\n\
The Azure CLI is available and **pre-configured for Azure DevOps reads**. You do not need to sign in, and no credential is present in this sandbox for you to use or leak.\n\
\n\
- **Available** — {group_list}, scoped to the current organization and project. These are **read-only**: listing and getting work, and the results are real. `az rest` and `az devops invoke` also work for Azure DevOps reads, so a catalogued endpoint without a dedicated command is still reachable.\n\
- **Not available** — creating, updating or deleting anything; reading secrets (service connections, variable groups, secure files, tokens, permissions); scopes not listed in the Azure DevOps read policy above; and every other `az` command group, including Azure Resource Manager (`az resource`, `az account`, `az group`) and Microsoft Graph (`az ad`).\n\
\n\
Requests outside that boundary are refused by a policy proxy, not by a misconfiguration — retrying, changing the URL, or trying to authenticate will not help. To *change* anything, emit a safe output instead; that is the supported path for writes.\n\
\n\
If a read you need is refused, file a `missing-tool` safe output naming `azure-cli` and the exact command, so the operator can extend the catalog rather than leaving you blocked.\n"
        )
    } else {
        "\n\
---\n\
\n\
## Azure CLI (`az`)\n\
\n\
The Azure CLI is available inside this sandbox at `/usr/bin/az`, but ado-aw does not inject an Azure or Azure DevOps credential into the sandbox:\n\
\n\
- **Azure DevOps** \u{2014} `az devops`, `az pipelines`, `az repos`, and `az boards` are not pre-authenticated. When configured, use the `azure-devops` MCP tools for authenticated ADO reads.\n\
- **Azure Resource Manager and Microsoft Graph** \u{2014} `az resource`, `az account`, `az group`, `az ad`, and authenticated `az rest` calls are not configured for agent use.\n\
- Do not sign in or place Azure credentials in the sandbox. Request a supported tool instead.\n\
\n\
If a command you need isn't covered above, file a `missing-tool` safe output naming `azure-cli` so the operator can extend coverage rather than blocking on it silently.\n"
            .to_string()
    };

    let script = format!(
        "cat >> \"/tmp/awf-tools/agent-prompt.md\" << 'AZURE_CLI_PROMPT_EOF'\n\
{body}\
AZURE_CLI_PROMPT_EOF\n\
\n\
echo \"Azure CLI prompt appended\"\n"
    );
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

    /// Front matter that enables the Azure DevOps tool, which is what pulls in
    /// the policy engine and therefore the wrapper.
    fn fm_proxied() -> FrontMatter {
        serde_yaml::from_str("name: t\ndescription: x\ntools:\n  azure-devops:\n    org: myorg\n")
            .expect("front matter parses")
    }

    fn wrapper_step(front_matter: &FrontMatter) -> Option<BashStep> {
        let ctx = CompileContext::for_test(front_matter);
        AzureCliExtension
            .declarations(&ctx)
            .unwrap()
            .agent_prepare_steps
            .into_iter()
            .filter_map(|step| match step {
                Step::Bash(b) if b.display_name.contains("az wrapper") => Some(b),
                _ => None,
            })
            .next()
    }

    fn policy_prompt_step(front_matter: &FrontMatter) -> Option<BashStep> {
        let ctx = CompileContext::for_test(front_matter);
        AzureCliExtension
            .declarations(&ctx)
            .unwrap()
            .agent_prepare_steps
            .into_iter()
            .filter_map(|step| match step {
                Step::Bash(b) if b.display_name.contains("policy prompt") => Some(b),
                _ => None,
            })
            .next()
    }

    #[test]
    fn the_wrapper_is_installed_only_when_traffic_is_policed() {
        // Without the policy engine there is nothing to redirect to, and
        // shadowing `az` would break it rather than contain it.
        assert!(wrapper_step(&fm()).is_none());
        assert!(wrapper_step(&fm_proxied()).is_some());
    }

    #[test]
    fn the_wrapper_directory_shadows_the_real_az() {
        // The file alone is not enough: the agent runs in a chroot, so only
        // PATH order decides which binary it actually invokes.
        let plain = fm();
        let ctx_plain = CompileContext::for_test(&plain);
        assert!(
            AzureCliExtension
                .declarations(&ctx_plain)
                .unwrap()
                .awf_path_prepends
                .is_empty()
        );

        let proxied = fm_proxied();
        let ctx = CompileContext::for_test(&proxied);
        assert_eq!(
            AzureCliExtension
                .declarations(&ctx)
                .unwrap()
                .awf_path_prepends,
            vec![AZ_WRAPPER_DIR.to_string()]
        );
    }

    #[test]
    fn the_wrapper_install_is_gated_on_az_being_present() {
        // With no `az` on the runner there is nothing for the wrapper to exec,
        // and shadowing a missing binary turns a clear "command not found"
        // into a confusing wrapper error.
        let step = wrapper_step(&fm_proxied()).expect("wrapper step");
        assert_eq!(
            step.condition,
            Some(Condition::Ne(
                Expr::Variable("AW_AZ_MOUNTS".to_string()),
                Expr::Literal(String::new()),
            ))
        );
    }

    #[test]
    fn the_installed_wrapper_is_executable_and_starts_with_a_shebang() {
        let step = wrapper_step(&fm_proxied()).expect("wrapper step");
        assert!(step.script.contains(&format!("chmod 755 '{AZ_WRAPPER_PATH}'")));
        // The heredoc body must not be indented: a shebang preceded by
        // whitespace is not a shebang, and the file would fail to exec.
        assert!(
            step.script.contains("ADO_AW_AZ_WRAPPER_EOF'\n#!/bin/sh"),
            "the wrapper body must start at column 0: {}",
            step.script
        );
        // A quoted heredoc delimiter keeps the shell from expanding `$PATH`,
        // `$@` and friends while writing the file.
        assert!(step.script.contains("<< 'ADO_AW_AZ_WRAPPER_EOF'"));
    }

    #[test]
    fn the_policy_prompt_is_present_even_when_az_is_not_detected() {
        let step = policy_prompt_step(&fm_proxied()).expect("policy prompt");
        assert!(
            step.condition.is_none(),
            "MCP reads use the same policy, so policy feedback must not depend on az detection"
        );
        assert!(step.script.contains("Enabled capabilities:"));
        assert!(step.script.contains("sanitized proxy decision logs"));
    }

    #[test]
    fn the_policy_prompt_lists_explicit_and_repository_only_scopes() {
        let mut front_matter = crate::compile::parse_markdown(
            r#"---
name: t
description: x
tools:
  azure-devops:
    org: myorg
permissions:
  read:
    service-connection: sc
    capabilities: [core, repos]
    allow:
      - organization: fabrikam
        projects:
          - project: Shared
            repositories: [shared-api]
repos:
  - LocalProject/implicit-api
---
"#,
        )
        .unwrap()
        .0;
        let (repositories, checkout, fetch) =
            crate::compile::resolve_repos(&front_matter).unwrap();
        front_matter.repositories = repositories;
        front_matter.checkout = checkout;
        front_matter.checkout_fetch = fetch;

        let step = policy_prompt_step(&front_matter).expect("policy prompt");
        assert!(step.script.contains("`fabrikam/Shared`"));
        assert!(step.script.contains("repositories: `shared-api`"));
        assert!(step.script.contains("Repository-only `LocalProject/implicit-api`"));
        assert!(step.script.contains("does **not** grant project"));
        assert!(step.script.contains("`discovery`, `core`, `repos`"));
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
        // names, auth boundary, and the missing-tool escape hatch. Style
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
            "not pre-authenticated",
            "azure-devops",
            "Do not sign in",
            "missing-tool",
        ] {
            assert!(
                append.script.contains(anchor),
                "prompt-append step must contain anchor `{anchor}`. Step:\n{}",
                append.script
            );
        }
        assert!(
            !append.script.contains("AZURE_DEVOPS_EXT_PAT"),
            "the Agent prompt must not claim the direct CLI receives an ADO credential"
        );
    }

    #[test]
    fn test_azure_cli_prompt_append_uses_single_quoted_heredoc() {
        // Keep the prompt heredoc non-expanding. Future advisory text may
        // contain environment-variable names, and changing this to an
        // unquoted delimiter could bake a secret into the agent prompt.
        let ext = AzureCliExtension;
        let fm = fm();
        let ctx = CompileContext::for_test(&fm);
        let steps = agent_prepare_steps(&ext, &ctx);
        let append = bash_step(&steps[1]);
        assert!(
            append.script.contains("<< 'AZURE_CLI_PROMPT_EOF'"),
            "prompt-append heredoc delimiter must be single-quoted to \
             prevent expansion of environment references inside the prompt \
             body. Step:\n{}",
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
