//! In-tree registry catalog for CLI consumers.

use std::error::Error;
use std::fmt;

use serde::Serialize;

use crate::engine::DEFAULT_COPILOT_MODEL;
use crate::safe_outputs::{ALL_KNOWN_SAFE_OUTPUTS, ALWAYS_ON_TOOLS, DEBUG_ONLY_TOOLS};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SafeOutputCatalogEntry {
    pub name: String,
    pub classification: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuntimeCatalogEntry {
    pub id: String,
    pub default_version: Option<String>,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolCatalogEntry {
    pub id: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Catalog {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub safe_outputs: Vec<SafeOutputCatalogEntry>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub runtimes: Vec<RuntimeCatalogEntry>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolCatalogEntry>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub engines: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownCatalogKind {
    pub kind: String,
}

impl fmt::Display for UnknownCatalogKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown --kind '{}' (expected one of: safe-outputs, runtimes, tools, engines, models)",
            self.kind
        )
    }
}

impl Error for UnknownCatalogKind {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogKind {
    SafeOutputs,
    Runtimes,
    Tools,
    Engines,
    Models,
}

impl CatalogKind {
    pub fn parse(kind: &str) -> Result<Self, UnknownCatalogKind> {
        match kind {
            "safe-outputs" => Ok(Self::SafeOutputs),
            "runtimes" => Ok(Self::Runtimes),
            "tools" => Ok(Self::Tools),
            "engines" => Ok(Self::Engines),
            "models" => Ok(Self::Models),
            other => Err(UnknownCatalogKind {
                kind: other.to_string(),
            }),
        }
    }
}

pub fn catalog() -> Catalog {
    Catalog {
        safe_outputs: safe_outputs(),
        runtimes: runtimes(),
        tools: tools(),
        engines: engines(),
        models: models(),
    }
}

pub fn catalog_kind(kind: &str) -> Result<Catalog, UnknownCatalogKind> {
    let kind = CatalogKind::parse(kind)?;
    Ok(match kind {
        CatalogKind::SafeOutputs => Catalog {
            safe_outputs: safe_outputs(),
            ..Catalog::default()
        },
        CatalogKind::Runtimes => Catalog {
            runtimes: runtimes(),
            ..Catalog::default()
        },
        CatalogKind::Tools => Catalog {
            tools: tools(),
            ..Catalog::default()
        },
        CatalogKind::Engines => Catalog {
            engines: engines(),
            ..Catalog::default()
        },
        CatalogKind::Models => Catalog {
            models: models(),
            ..Catalog::default()
        },
    })
}

pub fn render_text(catalog: &Catalog) -> String {
    let mut out = String::new();
    if !catalog.safe_outputs.is_empty() {
        out.push_str("Safe outputs\n");
        for item in &catalog.safe_outputs {
            out.push_str(&format!(
                "  {} [{}] - {}\n",
                item.name, item.classification, item.description
            ));
        }
        out.push('\n');
    }
    if !catalog.runtimes.is_empty() {
        out.push_str("Runtimes\n");
        for item in &catalog.runtimes {
            let version = item.default_version.as_deref().unwrap_or("none");
            out.push_str(&format!(
                "  {} [default: {}] - {}\n",
                item.id, version, item.description
            ));
        }
        out.push('\n');
    }
    if !catalog.tools.is_empty() {
        out.push_str("Tools\n");
        for item in &catalog.tools {
            out.push_str(&format!("  {} - {}\n", item.id, item.description));
        }
        out.push('\n');
    }
    if !catalog.engines.is_empty() {
        out.push_str("Engines\n");
        for engine in &catalog.engines {
            out.push_str(&format!("  {engine}\n"));
        }
        out.push('\n');
    }
    if !catalog.models.is_empty() {
        out.push_str("Models\n");
        for model in &catalog.models {
            out.push_str(&format!("  {model}\n"));
        }
    }
    out.trim_end().to_string()
}

fn safe_outputs() -> Vec<SafeOutputCatalogEntry> {
    ALL_KNOWN_SAFE_OUTPUTS
        .iter()
        .chain(DEBUG_ONLY_TOOLS.iter())
        .copied()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .map(|name| SafeOutputCatalogEntry {
            name: name.to_string(),
            classification: safe_output_classification(name).to_string(),
            description: safe_output_description(name).to_string(),
        })
        .collect()
}

fn safe_output_classification(name: &str) -> &'static str {
    if DEBUG_ONLY_TOOLS.contains(&name) {
        "debug-only"
    } else if ALWAYS_ON_TOOLS.contains(&name) {
        "always-on"
    } else {
        "opt-in"
    }
}

fn safe_output_description(name: &str) -> &'static str {
    match name {
        "add-build-tag" => "Parameters for adding a tag to an Azure DevOps build",
        "add-pr-comment" => "Parameters for adding a comment thread on a pull request",
        "comment-on-work-item" => "Parameters for commenting on a work item",
        "create-branch" => "Parameters for creating a branch",
        "create-git-tag" => "Parameters for creating a git tag (agent-provided)",
        "create-issue" => "Files a GitHub issue against an operator-configured target repository.",
        "create-pull-request" => "Parameters for creating a pull request",
        "create-wiki-page" => "Parameters for creating a wiki page (agent-provided)",
        "create-work-item" => "Parameters for creating a work item",
        "link-work-items" => "Parameters for linking two work items",
        "missing-data" => "Parameters for reporting missing data",
        "missing-tool" => "Parameters for reporting a missing tool",
        "noop" => "Parameters for describing a no operation. Use this if there is no work to do.",
        "queue-build" => "Parameters for queuing a build",
        "reply-to-pr-comment" => {
            "Parameters for replying to an existing review comment thread on a pull request"
        }
        "report-incomplete" => "Parameters for reporting that a task could not be completed",
        "resolve-pr-thread" => "Parameters for resolving or reactivating a PR review thread",
        "submit-pr-review" => "Parameters for submitting a pull request review",
        "update-pr" => "Parameters for updating a pull request",
        "update-wiki-page" => "Parameters for editing a wiki page (agent-provided)",
        "update-work-item" => "Parameters for updating a work item",
        "upload-build-attachment" => "Parameters for attaching a workspace file to an ADO build.",
        "upload-pipeline-artifact" => {
            "Parameters for publishing a workspace file as an ADO pipeline artifact."
        }
        "upload-workitem-attachment" => "Parameters for uploading an attachment to a work item",
        _ => "(no description)",
    }
}

fn runtimes() -> Vec<RuntimeCatalogEntry> {
    vec![
        RuntimeCatalogEntry {
            id: "lean".to_string(),
            default_version: Some("stable".to_string()),
            description: "Lean 4 runtime support for the ado-aw compiler.".to_string(),
        },
        RuntimeCatalogEntry {
            id: "python".to_string(),
            default_version: Some("3.x".to_string()),
            description: "Python runtime support for the ado-aw compiler.".to_string(),
        },
        RuntimeCatalogEntry {
            id: "node".to_string(),
            default_version: Some("22.x".to_string()),
            description: "Node.js runtime support for the ado-aw compiler.".to_string(),
        },
        RuntimeCatalogEntry {
            id: "dotnet".to_string(),
            default_version: Some("8.0.x".to_string()),
            description: ".NET runtime support for the ado-aw compiler.".to_string(),
        },
    ]
}

fn tools() -> Vec<ToolCatalogEntry> {
    vec![
        ToolCatalogEntry {
            id: "bash".to_string(),
            description: "Bash command access configured via tools.bash; omitted means unrestricted bash access.".to_string(),
        },
        ToolCatalogEntry {
            id: "edit".to_string(),
            description: "File writing configured via tools.edit; enabled by default.".to_string(),
        },
        ToolCatalogEntry {
            id: "azure-devops".to_string(),
            description: "Azure DevOps first-class tool.".to_string(),
        },
        ToolCatalogEntry {
            id: "cache-memory".to_string(),
            description: "Cache memory first-class tool.".to_string(),
        },
    ]
}

fn engines() -> Vec<String> {
    // TODO: Switch to an enum-driven Engine::all_ids() API when engine.rs exposes one.
    vec!["copilot".to_string()]
}

fn models() -> Vec<String> {
    // No KNOWN_MODELS registry exists yet; keep this list aligned with
    // prompts/create-ado-agentic-workflow.md step 2.
    vec![
        DEFAULT_COPILOT_MODEL.to_string(),
        "claude-sonnet-4.6".to_string(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_inspect_returns_non_empty_lists_for_every_category() {
        let catalog = catalog();
        assert!(!catalog.safe_outputs.is_empty());
        assert!(!catalog.runtimes.is_empty());
        assert!(!catalog.tools.is_empty());
        assert!(!catalog.engines.is_empty());
        assert!(!catalog.models.is_empty());
    }

    #[test]
    fn safe_outputs_inspect_catalog_kind_includes_always_on_tools() {
        let catalog = catalog_kind("safe-outputs").unwrap();
        let names: Vec<&str> = catalog
            .safe_outputs
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        for always_on in ALWAYS_ON_TOOLS {
            assert!(
                names.contains(always_on),
                "safe-outputs catalog missing always-on tool {always_on}"
            );
        }
    }

    #[test]
    fn unknown_inspect_catalog_kind_returns_typed_error() {
        let err = catalog_kind("widgets").unwrap_err();
        assert_eq!(err.kind, "widgets");
    }
}
