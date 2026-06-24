use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

// The agent template is embedded from src/data/init-agent.md
const AGENT_TEMPLATE: &str = include_str!("data/init-agent.md");

const AGENT_DIR: &str = ".github/agents";
const AGENT_FILENAME: &str = "ado-aw.agent.md";

/// Root directory (relative to the target repo) for the generated Agency /
/// Claude Code plugin. Mirrors the canonical in-repo layout (`agency/plugins/
/// ado-aw/`) so a scaffolded consumer repo matches how the plugin is checked in
/// to ado-aw itself, keeping `--agency` output and the source of truth aligned.
const AGENCY_PLUGIN_DIR: &str = "agency/plugins/ado-aw";

/// Marketplace catalog files written at the **repo root** (not inside the plugin
/// dir). These are what make the repository itself an installable marketplace:
/// `/plugin marketplace add <repo>` reads the root `.claude-plugin/marketplace
/// .json` (Claude) / `.github/plugin/marketplace.json` (Copilot), each of which
/// lists the `ado-aw` plugin with `source: "./agency/plugins/ado-aw"`. Written
/// verbatim from the canonical catalogs at the ado-aw repo root.
///
/// Each entry is `(path relative to the repo root, embedded file)`.
const AGENCY_MARKETPLACE_FILES: &[(&str, &str)] = &[
    (
        ".claude-plugin/marketplace.json",
        include_str!("../.claude-plugin/marketplace.json"),
    ),
    (
        ".github/plugin/marketplace.json",
        include_str!("../.github/plugin/marketplace.json"),
    ),
];

/// Files that make up the Agency / Claude Code plugin. The canonical, live copy
/// is checked in at `agency/plugins/ado-aw/` (the single source of truth, listed
/// in the Agency marketplace via an external `source` pointer and version-locked
/// to the compiler by release-please). `--agency` embeds that same tree and
/// scaffolds it into a consumer repo, so the two stay byte-for-byte identical.
///
/// Each entry is `(relative path within the plugin dir, embedded file)`. The
/// embedded files already carry the literal release version (release-please
/// bumps the canonical files and `Cargo.toml` together), so they are written
/// verbatim — no placeholder substitution.
const AGENCY_PLUGIN_FILES: &[(&str, &str)] = &[
    (
        ".claude-plugin/plugin.json",
        include_str!("../agency/plugins/ado-aw/.claude-plugin/plugin.json"),
    ),
    (
        ".mcp.json",
        include_str!("../agency/plugins/ado-aw/.mcp.json"),
    ),
    (
        "agency.json",
        include_str!("../agency/plugins/ado-aw/agency.json"),
    ),
    (
        "README.md",
        include_str!("../agency/plugins/ado-aw/README.md"),
    ),
    (
        "agents/ado-aw.md",
        include_str!("../agency/plugins/ado-aw/agents/ado-aw.md"),
    ),
    (
        "skills/create-workflow/SKILL.md",
        include_str!("../agency/plugins/ado-aw/skills/create-workflow/SKILL.md"),
    ),
    (
        "skills/update-workflow/SKILL.md",
        include_str!("../agency/plugins/ado-aw/skills/update-workflow/SKILL.md"),
    ),
    (
        "skills/debug-workflow/SKILL.md",
        include_str!("../agency/plugins/ado-aw/skills/debug-workflow/SKILL.md"),
    ),
    (
        "skills/compile-and-validate/SKILL.md",
        include_str!("../agency/plugins/ado-aw/skills/compile-and-validate/SKILL.md"),
    ),
    (
        "skills/manage-lifecycle/SKILL.md",
        include_str!("../agency/plugins/ado-aw/skills/manage-lifecycle/SKILL.md"),
    ),
    (
        "skills/audit-build/SKILL.md",
        include_str!("../agency/plugins/ado-aw/skills/audit-build/SKILL.md"),
    ),
    (
        "scripts/doctor.sh",
        include_str!("../agency/plugins/ado-aw/scripts/doctor.sh"),
    ),
    (
        "scripts/doctor.ps1",
        include_str!("../agency/plugins/ado-aw/scripts/doctor.ps1"),
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
        write_agency_plugin(&base).await?;
    }

    println!();
    println!("This agent helps you create, update, and debug Azure DevOps agentic workflows.");
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

/// Write the Agency / Claude Code plugin into `<base>/agency/plugins/ado-aw`,
/// plus the repo-root marketplace catalogs that make `<base>` an installable
/// marketplace.
///
/// The plugin is additive to the standard agent file and follows the Claude
/// Code plugin conventions, written under `agency/plugins/ado-aw` so it mirrors
/// how the plugin is checked in to ado-aw itself. Files are copied verbatim from
/// the canonical in-repo plugin (`agency/plugins/ado-aw/`); they already carry
/// the correct release version, so no substitution is performed.
///
/// The root catalogs (`.claude-plugin/marketplace.json`,
/// `.github/plugin/marketplace.json`) are what `/plugin marketplace add <repo>`
/// reads, so the scaffolded repo is directly registerable as a marketplace.
async fn write_agency_plugin(base: &Path) -> Result<()> {
    let plugin_root = base.join(AGENCY_PLUGIN_DIR);

    for (rel_path, contents) in AGENCY_PLUGIN_FILES {
        let dest = plugin_root.join(rel_path);
        let parent = dest
            .parent()
            .with_context(|| format!("Plugin file has no parent directory: {}", dest.display()))?;
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        tokio::fs::write(&dest, contents)
            .await
            .with_context(|| format!("Failed to write plugin file: {}", dest.display()))?;

        // `tokio::fs::write` creates files mode 0644 (minus umask); shell scripts
        // need the executable bit so the documented `./scripts/doctor.sh` works.
        #[cfg(unix)]
        if rel_path.ends_with(".sh") {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))
                .await
                .with_context(|| {
                    format!("Failed to set executable bit: {}", dest.display())
                })?;
        }
    }

    // Root marketplace catalogs (written relative to the repo root, not the
    // plugin dir) so `/plugin marketplace add <repo>` can detect the plugin.
    //
    // Unlike the plugin files under `agency/plugins/ado-aw/` (which ado-aw owns
    // and may freely re-scaffold), these root catalogs are shared repo-level
    // files a consumer might already maintain — e.g. when running `init
    // --agency` in a repo that is itself a plugin marketplace. Never silently
    // clobber an existing, differing catalog: skip it with a warning so the
    // user can merge by hand. Re-writing an identical catalog is a no-op, which
    // keeps `init --agency` idempotent.
    let mut wrote_catalog = false;
    let mut skipped_catalog = false;
    for (rel_path, contents) in AGENCY_MARKETPLACE_FILES {
        let dest = base.join(rel_path);

        if let Ok(existing) = tokio::fs::read_to_string(&dest).await {
            if existing == *contents {
                continue; // identical — idempotent no-op
            }
            eprintln!(
                "⚠ Skipped {rel_path}: a different marketplace catalog already exists. \
                 Merge the `ado-aw` plugin entry into it by hand (see docs/agency-plugin.md)."
            );
            skipped_catalog = true;
            continue;
        }

        let parent = dest.parent().with_context(|| {
            format!("Marketplace catalog has no parent directory: {}", dest.display())
        })?;
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        tokio::fs::write(&dest, contents)
            .await
            .with_context(|| format!("Failed to write marketplace catalog: {}", dest.display()))?;
        wrote_catalog = true;
    }

    println!("✓ Created Agency plugin in {}", plugin_root.display());
    if wrote_catalog {
        println!(
            "✓ Wrote marketplace catalogs (.claude-plugin/, .github/plugin/) — register with: /plugin marketplace add <this repo>"
        );
    } else if skipped_catalog {
        println!(
            "ℹ Left existing marketplace catalogs untouched — add the `ado-aw` plugin entry manually to register it."
        );
    }

    Ok(())
}
