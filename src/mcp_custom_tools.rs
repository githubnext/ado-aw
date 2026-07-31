//! Dynamic (config-driven) custom safe-output MCP tools.
//!
//! Built-in safe-output tools are registered statically via the rmcp
//! `#[tool_router]` macro. Custom safe-output jobs declared under
//! `safe-outputs.jobs.<name>` are not known at
//! compile time, so they are surfaced from a **generated schema file** loaded
//! at server startup.
//!
//! The compiler emits a JSON array of tool definitions (name + description +
//! closed input JSON Schema, `additionalProperties: false`) and passes its path
//! via `--custom-tools`. Each entry is registered as a real MCP tool whose
//! generic handler appends a proposal NDJSON line tagged with the tool name —
//! the same `{ "name": <tool>, <...args> }` shape the built-in tools produce.
//! The generated schema and per-run call budget are enforced before persistence.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use log::{info, warn};
use rmcp::ErrorData as McpError;
use rmcp::handler::server::router::tool::{ToolRoute, ToolRouter};
use rmcp::handler::server::tool::ToolCallContext;
use rmcp::model::{CallToolResult, Content, Tool};
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::mcp::SafeOutputs;

/// A single custom-tool definition as emitted by the compiler.
#[derive(Debug, Clone, Deserialize)]
pub struct CustomToolDef {
    /// The MCP tool name (also the proposal `name` tag).
    pub name: String,
    /// Human-readable description shown to the agent.
    #[serde(default)]
    pub description: String,
    /// Closed JSON Schema for the tool inputs (`additionalProperties: false`).
    #[serde(rename = "inputSchema", default)]
    pub input_schema: Map<String, Value>,
    #[serde(default = "default_custom_max")]
    pub max: usize,
    #[serde(default)]
    pub output: Option<String>,
}

fn default_custom_max() -> usize {
    crate::compile::custom_tools::DEFAULT_CUSTOM_MAX
}

/// Load custom-tool definitions from a compiler-generated JSON file.
///
/// The file is a JSON array of [`CustomToolDef`]. A missing file is an error
/// (the caller only passes a path when custom tools were configured).
pub fn load_custom_tool_defs(path: &Path) -> Result<Vec<CustomToolDef>> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read custom-tools file: {}", path.display()))?;
    let value: Value = serde_json::from_str(&contents)
        .with_context(|| format!("Failed to parse custom-tools JSON: {}", path.display()))?;
    let definitions = value.get("customTools").cloned().unwrap_or(value);
    let defs: Vec<CustomToolDef> = serde_json::from_value(definitions).with_context(|| {
        format!(
            "Failed to parse custom tool definitions: {}",
            path.display()
        )
    })?;
    Ok(defs)
}

/// Register each custom-tool definition as a dynamic route on `tool_router`.
///
/// A name that collides with an already-registered (built-in) tool is skipped
/// with a warning — built-ins are never shadowed by a custom tool.
pub fn register_custom_tools(tool_router: &mut ToolRouter<SafeOutputs>, defs: Vec<CustomToolDef>) {
    for def in defs {
        if tool_router.has_route(&def.name) {
            warn!(
                "Custom tool '{}' collides with an existing tool; skipping",
                def.name
            );
            continue;
        }
        tool_router.add_route(build_custom_route(def));
    }
}

/// Build a dynamic [`ToolRoute`] whose handler records the agent's inputs as a
/// proposal NDJSON line.
fn build_custom_route(def: CustomToolDef) -> ToolRoute<SafeOutputs> {
    let tool_name = def.name.clone();
    let schema: Arc<Map<String, Value>> = Arc::new(def.input_schema);
    let tool = Tool::new(def.name.clone(), def.description.clone(), schema.clone());
    let max = def.max;
    let output = def.output;

    ToolRoute::new_dyn(tool, move |ctx: ToolCallContext<'_, SafeOutputs>| {
        let tool_name = tool_name.clone();
        let schema = schema.clone();
        let output = output.clone();
        let output_path = ctx.service.custom_output_path();
        let proposal_lock = ctx.service.custom_proposal_lock();
        let arguments = ctx.arguments.clone();
        Box::pin(async move {
            let _guard = proposal_lock.lock().await;
            record_custom_proposal(
                &output_path,
                &tool_name,
                schema.as_ref(),
                max,
                output.as_deref(),
                arguments,
            )
            .await
        })
    })
}

async fn record_custom_proposal(
    output_path: &Path,
    tool_name: &str,
    schema: &Map<String, Value>,
    max: usize,
    acknowledgement: Option<&str>,
    arguments: Option<Map<String, Value>>,
) -> std::result::Result<CallToolResult, McpError> {
    let arguments =
        crate::compile::custom_tools::validate_custom_arguments(tool_name, schema, arguments)
            .map_err(|error| McpError::invalid_params(error.to_string(), None))?;
    let current = count_custom_proposals(output_path, tool_name)
        .await
        .map_err(|error| {
            warn!("Failed to inspect custom safe-output proposal budget '{tool_name}': {error:#}");
            McpError::internal_error(
                format!(
                    "Failed to inspect custom safe-output proposal '{}'",
                    crate::sanitize::sanitize_config(tool_name)
                ),
                None,
            )
        })?;
    if current >= max {
        return Err(McpError::invalid_params(
            format!(
                "Custom safe-output tool '{tool_name}' limit reached ({current} of {max} used)"
            ),
            None,
        ));
    }
    append_custom_proposal(output_path, tool_name, arguments)
        .await
        .map_err(|error| {
            warn!("Failed to record custom safe-output proposal '{tool_name}': {error:#}");
            McpError::internal_error(
                format!(
                    "Failed to record custom safe-output proposal '{}'",
                    crate::sanitize::sanitize_config(tool_name)
                ),
                None,
            )
        })?;
    Ok(CallToolResult::success(
        acknowledgement
            .map(|message| {
                vec![Content::text(
                    serde_json::json!({ "result": message }).to_string(),
                )]
            })
            .unwrap_or_default(),
    ))
}

/// Append a `{ "name": <tool>, <...args> }` proposal line to the NDJSON file.
async fn append_custom_proposal(
    output_path: &Path,
    tool_name: &str,
    arguments: Map<String, Value>,
) -> Result<()> {
    let mut entry = arguments;
    // The `name` tag is compiler-owned and always wins over any agent input.
    entry.insert("name".to_string(), Value::String(tool_name.to_string()));
    let line = serde_json::to_string(&Value::Object(entry))? + "\n";

    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(output_path)
        .await
        .with_context(|| {
            format!(
                "Failed to open NDJSON for append: {}",
                output_path.display()
            )
        })?;
    file.write_all(line.as_bytes()).await?;
    file.flush().await?;
    Ok(())
}

async fn count_custom_proposals(output_path: &Path, tool_name: &str) -> Result<usize> {
    if !tokio::fs::metadata(output_path)
        .await
        .map(|metadata| metadata.is_file())
        .unwrap_or(false)
    {
        return Ok(0);
    }
    Ok(crate::ndjson::read_ndjson_file(output_path)
        .await?
        .iter()
        .filter(|entry| entry.get("name").and_then(Value::as_str) == Some(tool_name))
        .count())
}

/// Load and register custom tools from `path`, logging a summary.
pub fn apply_custom_tools(tool_router: &mut ToolRouter<SafeOutputs>, path: &Path) -> Result<()> {
    let defs = load_custom_tool_defs(path)?;
    let count = defs.len();
    register_custom_tools(tool_router, defs);
    info!(
        "Registered {count} custom safe-output tool(s) from {}",
        path.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::custom_tools::{custom_tools_json, generate_custom_tool_schemas};
    use crate::compile::types::FrontMatter;

    const SAMPLE: &str = r#"[
      {
        "name": "send-notification",
        "description": "Send a structured notification.",
        "inputSchema": {
          "type": "object",
          "additionalProperties": false,
          "required": ["title"],
          "properties": {
            "title": { "type": "string" },
            "severity": { "type": "string", "enum": ["info", "warning", "critical"] }
          }
        },
        "max": 2,
        "output": "Accepted."
      }
    ]"#;

    fn parse_front_matter(yaml: &str) -> FrontMatter {
        serde_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn test_load_custom_tool_defs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("custom-tools.json");
        std::fs::write(&path, SAMPLE).unwrap();

        let defs = load_custom_tool_defs(&path).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "send-notification");
        assert_eq!(defs[0].description, "Send a structured notification.");
        assert_eq!(defs[0].max, 2);
        assert_eq!(defs[0].output.as_deref(), Some("Accepted."));
        assert_eq!(
            defs[0].input_schema["additionalProperties"],
            Value::Bool(false)
        );
    }

    #[test]
    fn test_register_custom_tools_adds_route() {
        let defs = serde_json::from_str::<Vec<CustomToolDef>>(SAMPLE).unwrap();
        let mut router: ToolRouter<SafeOutputs> = ToolRouter::new();
        register_custom_tools(&mut router, defs);
        assert!(router.has_route("send-notification"));
        assert_eq!(router.list_all().len(), 1);
    }

    #[test]
    fn test_generated_custom_tool_json_registers_only_declared_tool() {
        let fm = parse_front_matter(
            r#"
name: Test
description: Test
safe-outputs:
  jobs:
    send-notification:
      description: Send a structured notification.
      inputs:
        title:
          type: string
          description: Notification title.
          required: true
      steps:
        - bash: echo ok
"#,
        );
        let schemas = generate_custom_tool_schemas(&fm).unwrap();
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0].name, "send-notification");

        let empty_fm = parse_front_matter(
            r#"
name: Test
description: Test
"#,
        );
        assert!(generate_custom_tool_schemas(&empty_fm).unwrap().is_empty());

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("custom-tools.json");
        std::fs::write(&path, custom_tools_json(&schemas).unwrap()).unwrap();
        let defs = load_custom_tool_defs(&path).unwrap();

        let mut router: ToolRouter<SafeOutputs> = ToolRouter::new();
        register_custom_tools(&mut router, defs);
        assert!(router.has_route("send-notification"));
        assert!(!router.has_route("deploy-thing"));
    }

    #[test]
    fn test_register_skips_duplicate_name() {
        let mut router: ToolRouter<SafeOutputs> = ToolRouter::new();
        let defs = serde_json::from_str::<Vec<CustomToolDef>>(SAMPLE).unwrap();
        register_custom_tools(&mut router, defs);
        // Registering the same name again is a no-op (existing route wins).
        let dup = serde_json::from_str::<Vec<CustomToolDef>>(SAMPLE).unwrap();
        register_custom_tools(&mut router, dup);
        assert_eq!(router.list_all().len(), 1);
    }

    #[tokio::test]
    async fn test_append_custom_proposal_shape() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("safe_outputs.ndjson");
        let mut args = Map::new();
        args.insert("title".to_string(), Value::String("Outage".to_string()));
        append_custom_proposal(&path, "send-notification", args)
            .await
            .unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        let v: Value = serde_json::from_str(contents.trim()).unwrap();
        assert_eq!(v["name"], "send-notification");
        assert_eq!(v["title"], "Outage");
    }

    #[tokio::test]
    async fn custom_proposal_preserves_raw_value_until_stage3_materialization() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("safe_outputs.ndjson");
        let defs = serde_json::from_str::<Vec<CustomToolDef>>(SAMPLE).unwrap();
        let raw = "Outage\n##vso[task.setvariable variable=X]owned";

        record_custom_proposal(
            &path,
            &defs[0].name,
            &defs[0].input_schema,
            defs[0].max,
            defs[0].output.as_deref(),
            Some(Map::from_iter([(
                "title".to_string(),
                Value::String(raw.to_string()),
            )])),
        )
        .await
        .unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        let value: Value = serde_json::from_str(contents.trim()).unwrap();
        assert_eq!(value["title"], raw);
    }

    #[tokio::test]
    async fn test_record_custom_proposal_returns_mcp_error_on_persistence_failure() {
        let dir = tempfile::tempdir().unwrap();
        let output_path = dir.path().join("output-is-a-directory");
        std::fs::create_dir(&output_path).unwrap();

        let defs = serde_json::from_str::<Vec<CustomToolDef>>(SAMPLE).unwrap();
        let error = record_custom_proposal(
            &output_path,
            "send-notification",
            &defs[0].input_schema,
            defs[0].max,
            defs[0].output.as_deref(),
            Some(Map::from_iter([(
                "title".to_string(),
                Value::String("Outage".to_string()),
            )])),
        )
        .await
        .unwrap_err();

        assert_eq!(error.code, rmcp::model::ErrorCode::INTERNAL_ERROR);
        assert!(
            error
                .message
                .contains("Failed to record custom safe-output proposal")
        );
        assert!(
            !error.message.contains(&output_path.display().to_string()),
            "agent-visible MCP error must not expose the output path"
        );
    }

    #[tokio::test]
    async fn rejects_invalid_arguments_and_enforces_budget() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("safe_outputs.ndjson");
        let defs = serde_json::from_str::<Vec<CustomToolDef>>(SAMPLE).unwrap();
        let def = &defs[0];

        let error = record_custom_proposal(
            &path,
            &def.name,
            &def.input_schema,
            def.max,
            def.output.as_deref(),
            Some(Map::from_iter([(
                "extra".to_string(),
                Value::String("no".to_string()),
            )])),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, rmcp::model::ErrorCode::INVALID_PARAMS);

        for title in ["one", "two"] {
            record_custom_proposal(
                &path,
                &def.name,
                &def.input_schema,
                def.max,
                def.output.as_deref(),
                Some(Map::from_iter([(
                    "title".to_string(),
                    Value::String(title.to_string()),
                )])),
            )
            .await
            .unwrap();
        }
        let error = record_custom_proposal(
            &path,
            &def.name,
            &def.input_schema,
            def.max,
            def.output.as_deref(),
            Some(Map::from_iter([(
                "title".to_string(),
                Value::String("three".to_string()),
            )])),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    }
}
