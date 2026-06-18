use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

// The agent template is embedded from src/data/init-agent.md
const AGENT_TEMPLATE: &str = include_str!("data/init-agent.md");

const AGENT_DIR: &str = ".github/agents";
const AGENT_FILENAME: &str = "ado-aw.agent.md";

/// Root directory (relative to the target repo) for the generated Agency /
/// Claude Code plugin. Placed under `.github/ado-aw` so it does not pollute
/// the user's source tree like the conventional top-level plugin folder would.
const AGENCY_PLUGIN_DIR: &str = ".github/ado-aw";

/// Files that make up the Agency plugin, following the Claude Code plugin
/// conventions (https://code.claude.com/docs/en/plugins-reference):
///   - `.claude-plugin/plugin.json` — plugin manifest
///   - `.claude-plugin/marketplace.json` — marketplace so the plugin is
///     installable straight from this dir
///   - `agents/ado-aw.md` — dispatcher subagent
///   - `commands/*.md` — slash commands routing to prompts
///
/// Each entry is `(relative path within the plugin dir, embedded template)`.
/// `{{ compiler_version }}` placeholders are substituted at write time.
const AGENCY_PLUGIN_FILES: &[(&str, &str)] = &[
    (
        ".claude-plugin/plugin.json",
        include_str!("data/agency-plugin/.claude-plugin/plugin.json"),
    ),
    (
        ".claude-plugin/marketplace.json",
        include_str!("data/agency-plugin/.claude-plugin/marketplace.json"),
    ),
    (
        "agents/ado-aw.md",
        include_str!("data/agency-plugin/agents/ado-aw.md"),
    ),
    (
        "commands/create-ado-agentic-workflow.md",
        include_str!("data/agency-plugin/commands/create-ado-agentic-workflow.md"),
    ),
    (
        "commands/update-ado-agentic-workflow.md",
        include_str!("data/agency-plugin/commands/update-ado-agentic-workflow.md"),
    ),
    (
        "commands/debug-ado-agentic-workflow.md",
        include_str!("data/agency-plugin/commands/debug-ado-agentic-workflow.md"),
    ),
];

pub async fn run(path: Option<&std::path::Path>, agency: bool) -> Result<()> {
    let base = path
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let agent_dir = base.join(AGENT_DIR);
    let agent_path = agent_dir.join(AGENT_FILENAME);

    // `init` always (re)writes the agent file so it stays in sync with the
    // currently installed compiler version.

    // Create directory structure
    tokio::fs::create_dir_all(&agent_dir)
        .await
        .with_context(|| format!("Failed to create directory: {}", agent_dir.display()))?;

    // Substitute the pinned compiler version into the template
    let version = env!("CARGO_PKG_VERSION");
    let content = AGENT_TEMPLATE.replace("{{ compiler_version }}", version);

    // Write the agent file
    tokio::fs::write(&agent_path, content)
        .await
        .with_context(|| format!("Failed to write agent file: {}", agent_path.display()))?;

    // Print success message
    println!("✓ Created {}", agent_path.display());

    // `--agency` is additive: keep the standard agent file above and also emit
    // the Agency / Claude Code plugin.
    if agency {
        write_agency_plugin(&base, version).await?;
    }

    println!();
    println!("This agent helps you create, update, and debug Azure DevOps agentic pipelines.");
    println!("It will automatically download the ado-aw compiler and handle compilation.");
    println!();
    println!("To use it, ask your AI agent:");
    println!("  \"Create an ADO agentic workflow that <describe your workflow>\"");
    println!();
    println!("Or use the prompt directly with any AI agent:");
    println!(
        "  https://raw.githubusercontent.com/githubnext/ado-aw/v{version}/prompts/create-ado-agentic-workflow.md"
    );

    Ok(())
}

/// Write the Agency / Claude Code plugin into `<base>/.github/ado-aw`.
///
/// The plugin is additive to the standard agent file and follows the Claude
/// Code plugin conventions, written under `.github/ado-aw` so it does not
/// pollute the user's source tree.
async fn write_agency_plugin(base: &Path, version: &str) -> Result<()> {
    let plugin_root = base.join(AGENCY_PLUGIN_DIR);

    for (rel_path, template) in AGENCY_PLUGIN_FILES {
        let dest = plugin_root.join(rel_path);
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        }
        let content = template.replace("{{ compiler_version }}", version);
        tokio::fs::write(&dest, content)
            .await
            .with_context(|| format!("Failed to write plugin file: {}", dest.display()))?;
    }

    println!("✓ Created Agency plugin in {}", plugin_root.display());

    Ok(())
}
