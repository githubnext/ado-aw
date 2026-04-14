use anyhow::{Context, Result};
use std::path::PathBuf;

// The agent template is embedded from templates/init-agent.md
const AGENT_TEMPLATE: &str = include_str!("../templates/init-agent.md");

const AGENT_DIR: &str = ".github/agents";
const AGENT_FILENAME: &str = "ado-aw.agent.md";

pub async fn run(path: Option<&std::path::Path>, force: bool) -> Result<()> {
    let base = path.map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let agent_dir = base.join(AGENT_DIR);
    let agent_path = agent_dir.join(AGENT_FILENAME);

    // Check if file already exists
    if agent_path.exists() && !force {
        anyhow::bail!(
            "{} already exists. Use --force to overwrite.",
            agent_path.display()
        );
    }

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
    println!();
    println!("This agent helps you create, update, and debug Azure DevOps agentic pipelines.");
    println!("It will automatically download the ado-aw compiler and handle compilation.");
    println!();
    println!("To use it, ask your AI agent:");
    println!("  \"Create an ADO agentic workflow that <describe your workflow>\"");
    println!();
    println!("Or use the prompt directly with any AI agent:");
    println!("  https://raw.githubusercontent.com/githubnext/ado-aw/v{version}/prompts/create-ado-agentic-workflow.md");

    Ok(())
}
