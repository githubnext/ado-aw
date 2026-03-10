use anyhow::{Context, Result};
use inquire::{Confirm, InquireError, MultiSelect, Select, Text, error::InquireResult};
use log::{debug, info};
use std::fmt;
use std::path::PathBuf;

use crate::compile::sanitize_filename;

/// Available AI models for agent configuration
const AVAILABLE_MODELS: &[&str] = &[
    "claude-opus-4.5",
    "claude-sonnet-4.5",
    "gpt-5.2-codex",
    "gemini-3-pro-preview",
];

/// Configuration gathered from the interactive wizard
#[derive(Debug, Default)]
struct AgentConfig {
    name: String,
    description: String,
    model: String,
    schedule: Option<String>,
    branch: Option<String>,
    workspace: String,
    repositories: Vec<RepositoryConfig>,
    /// Repository aliases to checkout (if empty, all repos are checked out)
    checkout: Vec<String>,
    mcps: Vec<McpSelection>,
    prompt_body: String,
}

/// MCP selection with optional tool allow-list
#[derive(Debug, Clone)]
struct McpSelection {
    name: String,
    /// If None, all tools are allowed. If Some, only these tools are allowed.
    allowed_tools: Option<Vec<String>>,
}

#[derive(Debug)]
struct RepositoryConfig {
    alias: String,
    repo_type: String,
    name: String,
    ref_branch: String,
}

/// Wizard steps for navigation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WizardStep {
    Name,
    Description,
    Model,
    Schedule,
    Branch,
    Workspace,
    Repositories,
    Checkout,
    Mcps,
    Done,
}

impl WizardStep {
    fn next(self) -> Self {
        match self {
            Self::Name => Self::Description,
            Self::Description => Self::Model,
            Self::Model => Self::Schedule,
            Self::Schedule => Self::Branch,
            Self::Branch => Self::Workspace,
            Self::Workspace => Self::Repositories,
            Self::Repositories => Self::Checkout,
            Self::Checkout => Self::Mcps,
            Self::Mcps => Self::Done,
            Self::Done => Self::Done,
        }
    }

    fn prev(self) -> Self {
        match self {
            Self::Name => Self::Name,
            Self::Description => Self::Name,
            Self::Model => Self::Description,
            Self::Schedule => Self::Model,
            Self::Branch => Self::Schedule,
            Self::Workspace => Self::Branch,
            Self::Repositories => Self::Workspace,
            Self::Checkout => Self::Repositories,
            Self::Mcps => Self::Checkout,
            Self::Done => Self::Mcps,
        }
    }
}

/// Helper to handle prompt results with back navigation
fn handle_prompt<T>(result: InquireResult<T>, step: &mut WizardStep) -> Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(InquireError::OperationCanceled) => {
            let prev = step.prev();
            if prev == *step {
                // Already at the beginning, ask if user wants to quit
                let quit = Confirm::new("Exit wizard?")
                    .with_default(false)
                    .prompt()
                    .unwrap_or(false);
                if quit {
                    anyhow::bail!("Wizard cancelled by user");
                }
            } else {
                *step = prev;
            }
            Ok(None)
        }
        Err(InquireError::OperationInterrupted) => {
            anyhow::bail!("Wizard interrupted");
        }
        Err(e) => Err(e).context("Prompt failed"),
    }
}

/// Run the interactive agent creation wizard
pub async fn create_agent(output_dir: Option<PathBuf>) -> Result<()> {
    info!("Starting interactive agent creation wizard");
    debug!("Output directory: {:?}", output_dir);

    println!("\n🚀 Azure DevOps Agentic Pipeline Creator\n");
    println!("This wizard will guide you through creating a new agent configuration.");
    println!("Press Esc to go back to the previous step.\n");

    let mut config = AgentConfig::default();
    let mut step = WizardStep::Name;

    loop {
        match step {
            WizardStep::Name => {
                let prompt = Text::new("Agent Name:")
                    .with_help_message(
                        "Enter a human-readable name for your agent (Esc to go back)",
                    )
                    .with_default(&config.name)
                    .prompt();

                if let Some(name) = handle_prompt(prompt, &mut step)? {
                    config.name = name;
                    step = step.next();
                }
            }

            WizardStep::Description => {
                let prompt = Text::new("Description:")
                    .with_help_message(
                        "One-line description of what this agent does (Esc to go back)",
                    )
                    .with_default(&config.description)
                    .prompt();

                if let Some(desc) = handle_prompt(prompt, &mut step)? {
                    config.description = desc;
                    step = step.next();
                }
            }

            WizardStep::Model => {
                let default_idx = AVAILABLE_MODELS
                    .iter()
                    .position(|&m| m == config.model)
                    .unwrap_or(0);

                let prompt = Select::new("AI Model:", AVAILABLE_MODELS.to_vec())
                    .with_help_message("Select the AI model for this agent (Esc to go back)")
                    .with_starting_cursor(default_idx)
                    .prompt();

                if let Some(model) = handle_prompt(prompt, &mut step)? {
                    config.model = model.to_string();
                    step = step.next();
                }
            }

            WizardStep::Schedule => {
                match prompt_schedule_with_back(&mut step)? {
                    Some(schedule) => {
                        config.schedule = schedule;
                        step = step.next();
                    }
                    None => {
                        // User pressed Esc, step already updated by handle_prompt
                    }
                }
            }

            WizardStep::Branch => {
                let default_val = config.branch.clone().unwrap_or_default();
                let prompt = Text::new("Branch (optional):")
                    .with_help_message(
                        "Pin checkout to a specific branch (e.g., 'main'). Leave empty for default behavior. (Esc to go back)",
                    )
                    .with_default(&default_val)
                    .prompt();

                match prompt {
                    Ok(val) => {
                        config.branch = if val.trim().is_empty() {
                            None
                        } else {
                            Some(val.trim().to_string())
                        };
                        step = step.next();
                    }
                    Err(InquireError::OperationCanceled) => {
                        step = step.prev();
                    }
                    Err(e) => return Err(e).context("Failed to get branch"),
                }
            }

            WizardStep::Workspace => {
                let workspace_options = vec![
                    WorkspaceOption {
                        value: "root",
                        description: "Agent runs in $(Build.SourcesDirectory)",
                    },
                    WorkspaceOption {
                        value: "repo",
                        description: "Agent runs in $(Build.SourcesDirectory)/$(Build.Repository.Name)",
                    },
                ];

                let default_idx = workspace_options
                    .iter()
                    .position(|w| w.value == config.workspace)
                    .unwrap_or(0);

                let prompt = Select::new("Workspace:", workspace_options)
                    .with_help_message("Where should the agent execute? (Esc to go back)")
                    .with_starting_cursor(default_idx)
                    .prompt();

                if let Some(choice) = handle_prompt(prompt, &mut step)? {
                    config.workspace = choice.value.to_string();
                    step = step.next();
                }
            }

            WizardStep::Repositories => {
                match prompt_repositories_with_back(&mut config.repositories, &mut step)? {
                    true => step = step.next(),
                    false => { /* User went back, step already updated */ }
                }
            }

            WizardStep::Checkout => {
                match prompt_checkout_with_back(
                    &config.repositories,
                    &mut config.checkout,
                    &mut step,
                )? {
                    true => step = step.next(),
                    false => { /* User went back, step already updated */ }
                }
            }

            WizardStep::Mcps => {
                match prompt_mcps_with_back(&mut step)? {
                    Some(mcps) => {
                        config.mcps = mcps;
                        step = step.next();
                    }
                    None => {
                        // User pressed Esc, step already updated
                    }
                }
            }

            WizardStep::Done => break,
        }
    }

    info!("Agent wizard completed - generating markdown");
    debug!("Agent config: {:?}", config);

    // Generate the markdown file (user will edit instructions in the file directly)
    let markdown = generate_markdown(&config);

    // Determine output path
    let filename = sanitize_filename(&config.name);
    let output_path = output_dir
        .unwrap_or_else(|| PathBuf::from("."))
        .join(format!("{}.md", filename));

    info!("Writing agent file to: {}", output_path.display());

    // Create parent directories if they don't exist
    if let Some(parent) = output_path.parent() {
        if let Err(e) = tokio::fs::create_dir_all(parent).await {
            log::error!("Failed to create directory {}: {}", parent.display(), e);
            eprintln!(
                "\n❌ Failed to create directory {}: {}\n",
                parent.display(),
                e
            );
            eprintln!("Generated markdown:\n");
            eprintln!("{}", "─".repeat(60));
            eprintln!("{}", markdown);
            eprintln!("{}", "─".repeat(60));
            anyhow::bail!("Failed to create directory: {}", parent.display());
        }
    }

    if let Err(e) = tokio::fs::write(&output_path, &markdown).await {
        log::error!("Failed to write file {}: {}", output_path.display(), e);
        eprintln!(
            "\n❌ Failed to write file {}: {}\n",
            output_path.display(),
            e
        );
        eprintln!("Generated markdown:\n");
        eprintln!("{}", "─".repeat(60));
        eprintln!("{}", markdown);
        eprintln!("{}", "─".repeat(60));
        anyhow::bail!("Failed to write file: {}", output_path.display());
    }

    info!("Agent file created successfully: {}", output_path.display());
    println!("\n✅ Agent file created: {}", output_path.display());
    println!("\nNext steps:");
    println!("  1. Edit the file to add your agent instructions");
    println!(
        "  2. Compile with: ado-aw compile {}",
        output_path.display()
    );
    println!("  3. Commit both the .md and generated .yml files");

    Ok(())
}

/// Prompt for repositories with back navigation support
fn prompt_repositories_with_back(
    repositories: &mut Vec<RepositoryConfig>,
    step: &mut WizardStep,
) -> Result<bool> {
    let prompt = Confirm::new("Add additional repositories?")
        .with_default(false)
        .with_help_message(
            "Configure extra repositories for the agent to checkout (Esc to go back)",
        )
        .prompt();

    match prompt {
        Ok(false) => Ok(true), // No repos, proceed
        Ok(true) => {
            // Clear existing repos if re-entering this step
            repositories.clear();
            loop {
                match prompt_repository_with_back() {
                    Ok(Some(repo)) => {
                        repositories.push(repo);
                        let more = Confirm::new("Add another repository?")
                            .with_default(false)
                            .prompt()
                            .unwrap_or(false);
                        if !more {
                            break;
                        }
                    }
                    Ok(None) => {
                        // User cancelled during repo entry, go back to confirm
                        if repositories.is_empty() {
                            *step = step.prev();
                            return Ok(false);
                        }
                        // If we have some repos, just stop adding more
                        break;
                    }
                    Err(e) => return Err(e),
                }
            }
            Ok(true)
        }
        Err(InquireError::OperationCanceled) => {
            *step = step.prev();
            Ok(false)
        }
        Err(InquireError::OperationInterrupted) => {
            anyhow::bail!("Wizard interrupted");
        }
        Err(e) => Err(e).context("Failed to get confirmation"),
    }
}

/// Prompt for a single repository with back navigation
fn prompt_repository_with_back() -> Result<Option<RepositoryConfig>> {
    let name = match Text::new("Repository name (org/repo):")
        .with_help_message("e.g., my-org/my-repo (Esc to cancel)")
        .prompt()
    {
        Ok(n) => n,
        Err(InquireError::OperationCanceled) => return Ok(None),
        Err(e) => return Err(e).context("Failed to get repository name"),
    };

    let alias_default = name.split('/').last().unwrap_or(&name);
    let alias = Text::new("Alias:")
        .with_default(alias_default)
        .with_help_message("Short name to reference this repository")
        .prompt()
        .context("Failed to get alias")?;

    let repo_type = Text::new("Type:")
        .with_default("git")
        .prompt()
        .context("Failed to get type")?;

    let ref_branch = Text::new("Ref:")
        .with_default("refs/heads/main")
        .with_help_message("Branch reference")
        .prompt()
        .context("Failed to get ref")?;

    Ok(Some(RepositoryConfig {
        alias,
        repo_type,
        name,
        ref_branch,
    }))
}

/// Prompt for checkout selection with back navigation
/// Allows user to select which repositories the agent should checkout and work with
fn prompt_checkout_with_back(
    repositories: &[RepositoryConfig],
    checkout: &mut Vec<String>,
    step: &mut WizardStep,
) -> Result<bool> {
    // If no repositories configured, skip this step
    if repositories.is_empty() {
        return Ok(true);
    }

    let prompt = Confirm::new("Checkout additional repositories?")
        .with_default(false)
        .with_help_message(
            "By default, only 'self' is checked out. Select 'yes' to checkout additional repositories. (Esc to go back)",
        )
        .prompt();

    match prompt {
        Ok(false) => {
            // No additional repos checked out (only self)
            checkout.clear();
            Ok(true)
        }
        Ok(true) => {
            // Let user select which repos to checkout
            let repo_aliases: Vec<&str> = repositories.iter().map(|r| r.alias.as_str()).collect();

            let selected = MultiSelect::new(
                "Select repositories to checkout:",
                repo_aliases.clone(),
            )
            .with_help_message(
                "Space to select, Enter to confirm. These will be checked out alongside 'self'.",
            )
            .prompt();

            match selected {
                Ok(choices) => {
                    checkout.clear();
                    checkout.extend(choices.into_iter().map(String::from));
                    Ok(true)
                }
                Err(InquireError::OperationCanceled) => {
                    *step = step.prev();
                    Ok(false)
                }
                Err(InquireError::OperationInterrupted) => {
                    anyhow::bail!("Wizard interrupted");
                }
                Err(e) => Err(e).context("Failed to get checkout selection"),
            }
        }
        Err(InquireError::OperationCanceled) => {
            *step = step.prev();
            Ok(false)
        }
        Err(InquireError::OperationInterrupted) => {
            anyhow::bail!("Wizard interrupted");
        }
        Err(e) => Err(e).context("Failed to get confirmation"),
    }
}

/// Prompt for schedule with back navigation
fn prompt_schedule_with_back(step: &mut WizardStep) -> Result<Option<Option<String>>> {
    let frequency_options = vec![
        ScheduleOption {
            value: "none",
            description: "Manual or trigger-based only",
        },
        ScheduleOption {
            value: "hourly",
            description: "Every hour at a scattered minute",
        },
        ScheduleOption {
            value: "every_hours",
            description: "Every N hours (2, 3, 4, 6, 8, or 12)",
        },
        ScheduleOption {
            value: "every_minutes",
            description: "Every N minutes (5, 10, 15, 30)",
        },
        ScheduleOption {
            value: "daily",
            description: "Once per day",
        },
        ScheduleOption {
            value: "weekly",
            description: "Once per week",
        },
        ScheduleOption {
            value: "bi-weekly",
            description: "Every 14 days",
        },
        ScheduleOption {
            value: "tri-weekly",
            description: "Every 21 days",
        },
        ScheduleOption {
            value: "custom",
            description: "Enter custom fuzzy schedule expression",
        },
    ];

    let prompt = Select::new("Schedule Frequency:", frequency_options)
        .with_help_message("How often should this agent run? (Esc to go back)")
        .prompt();

    match prompt {
        Ok(frequency) => {
            let schedule = match frequency.value {
                "none" => None,
                "hourly" => Some("hourly".to_string()),
                "every_hours" => prompt_every_hours()?,
                "every_minutes" => prompt_every_minutes()?,
                "daily" => prompt_daily_schedule()?,
                "weekly" => prompt_weekly_schedule()?,
                "bi-weekly" => Some("bi-weekly".to_string()),
                "tri-weekly" => Some("tri-weekly".to_string()),
                "custom" => prompt_custom_schedule()?,
                _ => None,
            };
            Ok(Some(schedule))
        }
        Err(InquireError::OperationCanceled) => {
            *step = step.prev();
            Ok(None)
        }
        Err(InquireError::OperationInterrupted) => {
            anyhow::bail!("Wizard interrupted");
        }
        Err(e) => Err(e).context("Failed to select schedule frequency"),
    }
}

/// Prompt for MCPs with back navigation.
///
/// There are no built-in MCPs — all MCPs require explicit command configuration.
/// The wizard collects custom MCP names; command/args are configured in the
/// generated markdown front matter.
fn prompt_mcps_with_back(step: &mut WizardStep) -> Result<Option<Vec<McpSelection>>> {
    println!("\n🔧 MCP Server Configuration");
    println!("Add custom MCP servers. Each requires a command and args in the front matter.");
    println!("You can add MCP servers later by editing the generated markdown file.\n");

    let add_mcps = match Confirm::new("Would you like to add any custom MCP servers?")
        .with_default(false)
        .prompt()
    {
        Ok(val) => val,
        Err(InquireError::OperationCanceled) => {
            *step = step.prev();
            return Ok(None);
        }
        Err(InquireError::OperationInterrupted) => {
            anyhow::bail!("Wizard interrupted");
        }
        Err(e) => return Err(e).context("Failed to prompt for MCPs"),
    };

    if !add_mcps {
        return Ok(Some(Vec::new()));
    }

    let mut selections = Vec::new();
    loop {
        let name = match Text::new("MCP server name (or empty to finish):")
            .with_help_message("e.g., my-custom-tool")
            .prompt()
        {
            Ok(name) if name.trim().is_empty() => break,
            Ok(name) => name.trim().to_string(),
            Err(InquireError::OperationCanceled) => break,
            Err(InquireError::OperationInterrupted) => {
                anyhow::bail!("Wizard interrupted");
            }
            Err(e) => return Err(e).context("Failed to read MCP name"),
        };

        selections.push(McpSelection {
            name,
            allowed_tools: None,
        });
    }

    if !selections.is_empty() {
        println!("\n📋 Added {} custom MCP(s):", selections.len());
        for mcp in &selections {
            println!("   {} (configure command/args in front matter)", mcp.name);
        }
    }

    Ok(Some(selections))
}

/// Workspace option for display
struct WorkspaceOption {
    value: &'static str,
    description: &'static str,
}

impl fmt::Display for WorkspaceOption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} - {}", self.value, self.description)
    }
}

/// Schedule frequency option for display
struct ScheduleOption {
    value: &'static str,
    description: &'static str,
}

impl fmt::Display for ScheduleOption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} - {}", self.value, self.description)
    }
}

/// Weekday option for display
struct WeekdayOption {
    value: &'static str,
    display: &'static str,
}

impl fmt::Display for WeekdayOption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display)
    }
}

/// Time constraint option for display
struct TimeConstraintOption {
    value: &'static str,
    description: &'static str,
}

impl fmt::Display for TimeConstraintOption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} - {}", self.value, self.description)
    }
}

/// Prompt for "every N hours" schedule
fn prompt_every_hours() -> Result<Option<String>> {
    let hour_options = vec!["2", "3", "4", "6", "8", "12"];
    let hours = Select::new("Every how many hours?", hour_options)
        .with_help_message("Valid intervals that divide evenly into 24")
        .prompt()
        .context("Failed to select hours")?;

    Ok(Some(format!("every {}h", hours)))
}

/// Prompt for "every N minutes" schedule
fn prompt_every_minutes() -> Result<Option<String>> {
    let minute_options = vec!["5", "10", "15", "30"];
    let minutes = Select::new("Every how many minutes?", minute_options)
        .with_help_message("Minimum 5 minutes (platform constraint)")
        .prompt()
        .context("Failed to select minutes")?;

    Ok(Some(format!("every {} minutes", minutes)))
}

/// Prompt for daily schedule with time constraint options
fn prompt_daily_schedule() -> Result<Option<String>> {
    let constraint = prompt_time_constraint()?;

    match constraint.as_str() {
        "" => Ok(Some("daily".to_string())),
        c => Ok(Some(format!("daily {}", c))),
    }
}

/// Prompt for weekly schedule with day and time constraint options
fn prompt_weekly_schedule() -> Result<Option<String>> {
    let weekday_options = vec![
        WeekdayOption {
            value: "any",
            display: "Any day (scattered)",
        },
        WeekdayOption {
            value: "monday",
            display: "Monday",
        },
        WeekdayOption {
            value: "tuesday",
            display: "Tuesday",
        },
        WeekdayOption {
            value: "wednesday",
            display: "Wednesday",
        },
        WeekdayOption {
            value: "thursday",
            display: "Thursday",
        },
        WeekdayOption {
            value: "friday",
            display: "Friday",
        },
        WeekdayOption {
            value: "saturday",
            display: "Saturday",
        },
        WeekdayOption {
            value: "sunday",
            display: "Sunday",
        },
    ];

    let weekday = Select::new("Which day of the week?", weekday_options)
        .with_help_message("Select a specific day or let it scatter across the week")
        .prompt()
        .context("Failed to select weekday")?;

    let constraint = prompt_time_constraint()?;

    let schedule = match (weekday.value, constraint.as_str()) {
        ("any", "") => "weekly".to_string(),
        ("any", c) => format!("weekly {}", c),
        (day, "") => format!("weekly on {}", day),
        (day, c) => format!("weekly on {} {}", day, c),
    };

    Ok(Some(schedule))
}

/// Prompt for time constraint (around/between/none)
fn prompt_time_constraint() -> Result<String> {
    let constraint_options = vec![
        TimeConstraintOption {
            value: "none",
            description: "Scattered across the full period",
        },
        TimeConstraintOption {
            value: "around",
            description: "Around a specific time (±60 minutes)",
        },
        TimeConstraintOption {
            value: "between",
            description: "Between two times (e.g., business hours)",
        },
    ];

    let constraint = Select::new("Time constraint:", constraint_options)
        .with_help_message("Optionally constrain when the agent runs")
        .prompt()
        .context("Failed to select time constraint")?;

    match constraint.value {
        "none" => Ok(String::new()),
        "around" => {
            let time = Text::new("Around what time?")
                .with_help_message("e.g., 14:00, 3pm, noon, midnight")
                .prompt()
                .context("Failed to get time")?;

            let timezone = prompt_optional_timezone()?;
            Ok(format!("around {}{}", time, timezone))
        }
        "between" => {
            let start = Text::new("Start time:")
                .with_help_message("e.g., 9:00, 9am")
                .prompt()
                .context("Failed to get start time")?;

            let end = Text::new("End time:")
                .with_help_message("e.g., 17:00, 5pm")
                .prompt()
                .context("Failed to get end time")?;

            let timezone = prompt_optional_timezone()?;
            Ok(format!(
                "between {}{} and {}{}",
                start, timezone, end, timezone
            ))
        }
        _ => Ok(String::new()),
    }
}

/// Prompt for optional timezone offset
fn prompt_optional_timezone() -> Result<String> {
    let add_tz = Confirm::new("Add timezone offset?")
        .with_default(false)
        .with_help_message("Specify a UTC offset (e.g., utc+9 for JST, utc-5 for EST)")
        .prompt()
        .context("Failed to get timezone confirmation")?;

    if add_tz {
        let tz = Text::new("UTC offset:")
            .with_help_message("e.g., utc+9, utc-5, utc+05:30")
            .prompt()
            .context("Failed to get timezone")?;
        Ok(format!(" {}", tz))
    } else {
        Ok(String::new())
    }
}

/// Prompt for custom fuzzy schedule expression
fn prompt_custom_schedule() -> Result<Option<String>> {
    println!("\n📚 Fuzzy Schedule Syntax Examples:");
    println!("   daily                          - Scattered across full day");
    println!("   daily around 14:00             - Within ±60 min of 2 PM");
    println!("   daily between 9:00 and 17:00   - Business hours");
    println!("   weekly on monday               - Every Monday, scattered time");
    println!("   weekly on friday around 17:00  - Friday evenings");
    println!("   hourly                         - Every hour");
    println!("   every 6h                       - Every 6 hours");
    println!("   every 15 minutes               - Every 15 minutes");
    println!("   bi-weekly                      - Every 14 days");
    println!("   daily around 14:00 utc+9       - With timezone offset\n");

    let schedule = Text::new("Enter schedule expression:")
        .with_help_message("See examples above or refer to fuzzy schedule documentation")
        .prompt()
        .context("Failed to get custom schedule")?;

    if schedule.trim().is_empty() {
        Ok(None)
    } else {
        Ok(Some(schedule))
    }
}

/// Generate the markdown file content from the configuration
fn generate_markdown(config: &AgentConfig) -> String {
    let mut yaml_parts = Vec::new();

    // Name and description
    yaml_parts.push(format!("name: \"{}\"", escape_yaml_string(&config.name)));
    yaml_parts.push(format!(
        "description: \"{}\"",
        escape_yaml_string(&config.description)
    ));

    // Engine (only if not default)
    if config.model != "claude-opus-4.5" {
        yaml_parts.push(format!("engine: {}", config.model));
    }

    // Schedule
    if let Some(ref schedule) = config.schedule {
        yaml_parts.push(format!("schedule: {}", schedule));
    }

    // Branch (only if set)
    if let Some(ref branch) = config.branch {
        yaml_parts.push(format!("branch: {}", branch));
    }

    // Workspace (only if not default)
    if config.workspace != "root" {
        yaml_parts.push(format!("workspace: {}", config.workspace));
    }

    // Repositories
    if !config.repositories.is_empty() {
        yaml_parts.push("repositories:".to_string());
        for repo in &config.repositories {
            yaml_parts.push(format!("  - repository: {}", repo.alias));
            yaml_parts.push(format!("    type: {}", repo.repo_type));
            yaml_parts.push(format!("    name: {}", repo.name));
            if repo.ref_branch != "refs/heads/main" {
                yaml_parts.push(format!("    ref: {}", repo.ref_branch));
            }
        }
    }

    // Checkout (only if not all repos)
    if !config.checkout.is_empty() {
        yaml_parts.push("checkout:".to_string());
        for alias in &config.checkout {
            yaml_parts.push(format!("  - {}", alias));
        }
    }

    // MCP servers
    if !config.mcps.is_empty() {
        yaml_parts.push("mcp-servers:".to_string());
        for mcp in &config.mcps {
            match &mcp.allowed_tools {
                None => {
                    // All tools allowed
                    yaml_parts.push(format!("  {}: true", mcp.name));
                }
                Some(tools) if tools.is_empty() => {
                    // No tools selected - skip this MCP
                    continue;
                }
                Some(tools) => {
                    // Specific tools allowed
                    yaml_parts.push(format!("  {}:", mcp.name));
                    yaml_parts.push("    allowed:".to_string());
                    for tool in tools {
                        yaml_parts.push(format!("      - {}", tool));
                    }
                }
            }
        }
    }

    // Build the full markdown
    let mut markdown = String::new();
    markdown.push_str("---\n");
    markdown.push_str(&yaml_parts.join("\n"));
    markdown.push_str("\n---\n\n");

    // Add the prompt body
    if config.prompt_body.is_empty() {
        markdown.push_str(&format!("## {}\n\n", config.name));
        markdown.push_str("<!-- Add your agent instructions here -->\n\n");
        markdown.push_str("### Tasks\n\n");
        markdown.push_str("1. TODO: Define agent tasks\n");
    } else {
        markdown.push_str(&config.prompt_body);
        markdown.push('\n');
    }

    markdown
}

/// Escape special characters in YAML strings
fn escape_yaml_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_filename() {
        // Tests the imported sanitize_filename from compile::common
        assert_eq!(sanitize_filename("Daily Code Review"), "daily-code-review");
        assert_eq!(sanitize_filename("My Agent!"), "my-agent");
        assert_eq!(
            sanitize_filename("Test  Multiple   Spaces"),
            "test-multiple-spaces"
        );
    }

    #[test]
    fn test_escape_yaml_string() {
        assert_eq!(escape_yaml_string("hello"), "hello");
        assert_eq!(escape_yaml_string("say \"hello\""), "say \\\"hello\\\"");
    }

    #[test]
    fn test_generate_markdown_minimal() {
        let config = AgentConfig {
            name: "Test Agent".to_string(),
            description: "A test agent".to_string(),
            model: "claude-opus-4.5".to_string(),
            workspace: "root".to_string(),
            ..Default::default()
        };

        let markdown = generate_markdown(&config);
        assert!(markdown.contains("name: \"Test Agent\""));
        assert!(markdown.contains("description: \"A test agent\""));
        assert!(!markdown.contains("model:")); // Default model shouldn't appear
        assert!(!markdown.contains("workspace:")); // Default workspace shouldn't appear
    }

    #[test]
    fn test_generate_markdown_with_mcps() {
        let config = AgentConfig {
            name: "Test Agent".to_string(),
            description: "A test agent".to_string(),
            model: "claude-opus-4.5".to_string(),
            workspace: "root".to_string(),
            mcps: vec![
                McpSelection {
                    name: "ado".to_string(),
                    allowed_tools: None,
                },
                McpSelection {
                    name: "kusto".to_string(),
                    allowed_tools: None,
                },
            ],
            ..Default::default()
        };

        let markdown = generate_markdown(&config);
        assert!(markdown.contains("mcp-servers:"));
        assert!(markdown.contains("ado: true"));
        assert!(markdown.contains("kusto: true"));
    }

    #[test]
    fn test_generate_markdown_with_allowed_tools() {
        let config = AgentConfig {
            name: "Test Agent".to_string(),
            description: "A test agent".to_string(),
            model: "claude-opus-4.5".to_string(),
            workspace: "root".to_string(),
            mcps: vec![
                McpSelection {
                    name: "ado".to_string(),
                    allowed_tools: None, // All tools
                },
                McpSelection {
                    name: "icm".to_string(),
                    allowed_tools: Some(vec![
                        "create_incident".to_string(),
                        "get_incident".to_string(),
                    ]),
                },
            ],
            ..Default::default()
        };

        let markdown = generate_markdown(&config);
        assert!(markdown.contains("mcp-servers:"));
        assert!(markdown.contains("ado: true"));
        assert!(markdown.contains("icm:"));
        assert!(markdown.contains("allowed:"));
        assert!(markdown.contains("- create_incident"));
        assert!(markdown.contains("- get_incident"));
    }
}
