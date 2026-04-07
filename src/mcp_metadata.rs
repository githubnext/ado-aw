//! MCP Metadata - Bundled tool definitions for copilot MCPs
//!
//! This module provides access to pre-discovered MCP tool metadata that is
//! embedded at compile time. The metadata is refreshed by running:
//!
//! ```bash
//! # On Windows
//! ./refresh-mcp-metadata.ps1
//!
//! # On Linux/macOS
//! ./refresh-mcp-metadata.sh
//! ```
//!
//! The scripts query each built-in copilot MCP and save the tool definitions
//! to `mcp-metadata.json`, which is then embedded into the binary.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Bundled MCP metadata (embedded at compile time)
const BUNDLED_METADATA: &str = include_str!("../mcp-metadata.json");

/// Metadata for a single tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolMetadata {
    /// Tool name (without namespace prefix)
    pub name: String,
    /// Human-readable description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON schema for input parameters
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<serde_json::Value>,
}

/// Metadata for an MCP server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpMetadata {
    /// Server name/identifier
    pub name: String,
    /// Whether this is a built-in copilot MCP
    #[serde(default)]
    pub builtin: bool,
    /// Available tools
    #[serde(default)]
    pub tools: Vec<ToolMetadata>,
    /// When this metadata was last refreshed (ISO 8601)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refreshed_at: Option<String>,
    /// Error message if discovery failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Collection of MCP metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpMetadataFile {
    /// Schema version for forward compatibility
    pub version: String,
    /// When this file was generated
    pub generated_at: String,
    /// Metadata for each MCP server
    pub mcps: HashMap<String, McpMetadata>,
}

impl McpMetadataFile {
    /// Load the bundled metadata (embedded at compile time)
    pub fn bundled() -> Self {
        serde_json::from_str(BUNDLED_METADATA)
            .expect("Bundled mcp-metadata.json should be valid JSON")
    }

    /// Get metadata for a specific MCP
    pub fn get(&self, mcp_name: &str) -> Option<&McpMetadata> {
        self.mcps.get(mcp_name)
    }

    /// Get tools for a specific MCP
    pub fn get_tools(&self, mcp_name: &str) -> Option<&[ToolMetadata]> {
        self.mcps.get(mcp_name).map(|m| m.tools.as_slice())
    }

    /// Check if a tool exists for an MCP
    pub fn has_tool(&self, mcp_name: &str, tool_name: &str) -> bool {
        self.mcps
            .get(mcp_name)
            .map(|m| m.tools.iter().any(|t| t.name == tool_name))
            .unwrap_or(false)
    }

    /// Get all known MCP names
    pub fn mcp_names(&self) -> Vec<&str> {
        self.mcps.keys().map(|s| s.as_str()).collect()
    }

    /// Get all built-in MCP names (sorted alphabetically)
    pub fn builtin_mcp_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .mcps
            .iter()
            .filter(|(_, m)| m.builtin)
            .map(|(k, _)| k.as_str())
            .collect();
        names.sort();
        names
    }

    /// Get all tool names for an MCP (useful for validation)
    pub fn tool_names(&self, mcp_name: &str) -> Vec<&str> {
        self.mcps
            .get(mcp_name)
            .map(|m| m.tools.iter().map(|t| t.name.as_str()).collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bundled_metadata_loads() {
        let metadata = McpMetadataFile::bundled();
        assert_eq!(metadata.version, "1.0");
        // Should have the known built-in MCPs
        assert!(metadata.mcps.contains_key("ado"));
        assert!(metadata.mcps.contains_key("icm"));
        assert!(metadata.mcps.contains_key("kusto"));
    }

    #[test]
    fn test_get_mcp() {
        let metadata = McpMetadataFile::bundled();
        let ado = metadata.get("ado");
        assert!(ado.is_some());
        assert!(ado.unwrap().builtin);
    }

    #[test]
    fn test_mcp_names() {
        let metadata = McpMetadataFile::bundled();
        let names = metadata.mcp_names();
        assert!(names.contains(&"ado"));
        assert!(names.contains(&"icm"));
    }
}
