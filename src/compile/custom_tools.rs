//! Compile-time modeling and MCP schema generation for custom safe-output jobs.

use std::collections::HashSet;

use anyhow::{Context, Result, anyhow, bail, ensure};
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::compile::types::FrontMatter;
use crate::secure::CommitSha;

pub const DEFAULT_CUSTOM_MAX: usize = 1;
pub const CUSTOM_STRING_INPUT_MAX_BYTES: usize = 10 * 1024;

pub const COMPONENT_PROVENANCE_KEYS: [&str; 4] = [
    "component-source",
    "component-ref",
    "component-sha",
    "manifest-digest",
];

const JOB_KEYS: &[&str] = &[
    "display-name",
    "description",
    "condition",
    "needs",
    "timeout-minutes",
    "max",
    "inputs",
    "env",
    "output",
    "steps",
    "component-source",
    "component-ref",
    "component-sha",
    "manifest-digest",
];

const INPUT_KEYS: &[&str] = &["description", "required", "default", "type", "options"];

const COMPILER_ENV_KEYS: &[&str] = &["ADO_AW_AGENT_OUTPUT", "ADO_AW_SAFE_OUTPUTS_STAGED"];
const COMPILER_INPUT_KEYS: &[&str] = &["name", "type"];
pub(crate) const CUSTOM_JOB_SYSTEM_NEEDS: &[&str] = &[
    "agent",
    "detection",
    "safe-outputs",
    "safe-outputs-reviewed",
];

/// A compiler-generated custom MCP tool definition.
#[derive(Debug, Clone, PartialEq)]
pub struct CustomToolSchema {
    pub name: String,
    pub description: String,
    pub input_schema: Map<String, Value>,
    pub max: usize,
    pub output: Option<String>,
}

/// Compile-time provenance for a remotely imported custom job.
///
/// This is carried beside the typed job definition. It is never authorable
/// front matter and does not imply a runtime component checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomComponentDefinition {
    pub source: String,
    pub requested_ref: Option<String>,
    pub sha: CommitSha,
    pub manifest_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CustomToolDefinition {
    pub name: String,
    pub display_name: Option<String>,
    pub description: String,
    pub input_schema: Map<String, Value>,
    pub schema_digest: String,
    pub max: usize,
    pub env: Vec<(String, String)>,
    pub steps: Vec<Value>,
    pub condition: Option<String>,
    pub needs: Vec<String>,
    pub timeout_minutes: Option<u32>,
    pub output: Option<String>,
    pub component: Option<CustomComponentDefinition>,
}

pub fn collect_custom_tool_definitions(
    front_matter: &FrontMatter,
) -> Result<Vec<CustomToolDefinition>> {
    if front_matter.safe_outputs.contains_key("scripts") {
        bail!(
            "safe-outputs.scripts is not supported; use a self-contained \
             safe-outputs.jobs executor"
        );
    }

    let Some(section_value) = front_matter.safe_outputs.get("jobs") else {
        return Ok(Vec::new());
    };
    let jobs = section_value
        .as_object()
        .ok_or_else(|| anyhow!("safe-outputs.jobs must be a mapping of job name to definition"))?;

    let mut names: Vec<&String> = jobs.keys().collect();
    names.sort();
    let mut normalized_names = HashSet::new();
    let mut definitions = Vec::with_capacity(names.len());

    for tool_name in names {
        validate_tool_name(tool_name)?;
        let normalized = ado_identifier_suffix(tool_name);
        ensure!(
            normalized_names.insert(normalized),
            "custom safe-output tool '{tool_name}' collides with another tool after ADO \
             identifier normalization"
        );

        let tool_obj = jobs[tool_name]
            .as_object()
            .ok_or_else(|| anyhow!("safe-outputs.jobs.{tool_name} must be a mapping"))?;
        reject_unknown_keys(
            tool_obj,
            JOB_KEYS,
            &format!("safe-outputs.jobs.{tool_name}"),
        )?;

        let description = required_nonempty_string(
            tool_obj,
            "description",
            &format!("safe-outputs.jobs.{tool_name}.description"),
        )?;
        let display_name = optional_string(tool_obj, "display-name")
            .with_context(|| format!("safe-outputs.jobs.{tool_name}.display-name"))?;
        let condition = optional_string(tool_obj, "condition")
            .with_context(|| format!("safe-outputs.jobs.{tool_name}.condition"))?;
        let output = optional_string(tool_obj, "output")
            .with_context(|| format!("safe-outputs.jobs.{tool_name}.output"))?;
        if let Some(output) = output.as_deref() {
            ensure_agent_visible_literal(output, &format!("safe-outputs.jobs.{tool_name}.output"))?;
        }
        let needs = parse_needs(tool_obj.get("needs"), tool_name)?;
        let timeout_minutes = parse_timeout(tool_obj.get("timeout-minutes"), tool_name)?;
        let max = parse_max(tool_name, tool_obj.get("max"))?;
        let env = parse_env(tool_name, tool_obj.get("env"))?;
        let steps = tool_obj
            .get("steps")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("safe-outputs.jobs.{tool_name}.steps must be a list"))?
            .clone();
        ensure!(
            !steps.is_empty(),
            "safe-outputs.jobs.{tool_name}.steps must not be empty"
        );

        let input_schema = build_input_schema(tool_name, tool_obj.get("inputs"))?;
        let schema_digest = crate::hash::sha256_hex(
            &serde_json::to_vec(&input_schema)
                .context("failed to serialize custom tool schema for digest")?,
        );
        let component = parse_component(tool_obj, tool_name)?;

        definitions.push(CustomToolDefinition {
            name: tool_name.clone(),
            display_name,
            description,
            input_schema,
            schema_digest,
            max,
            env,
            steps,
            condition,
            needs,
            timeout_minutes,
            output,
            component,
        });
    }

    Ok(definitions)
}

/// Reject compiler-owned component provenance in authored front matter before
/// imports are resolved.
pub fn reject_author_component_provenance(front_matter: &FrontMatter) -> Result<()> {
    for section in ["scripts", "jobs"] {
        let Some(tools) = front_matter
            .safe_outputs
            .get(section)
            .and_then(Value::as_object)
        else {
            continue;
        };
        for (tool_name, tool_value) in tools {
            let Some(tool) = tool_value.as_object() else {
                continue;
            };
            for key in COMPONENT_PROVENANCE_KEYS {
                ensure!(
                    !tool.contains_key(key),
                    "safe-outputs.{section}.{tool_name}.{key} is compiler-owned and may only \
                     be supplied by a resolved remote import"
                );
            }
        }
    }
    Ok(())
}

pub fn generate_custom_tool_schemas(front_matter: &FrontMatter) -> Result<Vec<CustomToolSchema>> {
    collect_custom_tool_definitions(front_matter).map(|definitions| {
        definitions
            .into_iter()
            .map(|definition| CustomToolSchema {
                name: definition.name,
                description: definition.description,
                input_schema: definition.input_schema,
                max: definition.max,
                output: definition.output,
            })
            .collect()
    })
}

/// Serialize the dynamic MCP tool configuration.
pub fn custom_tools_json(schemas: &[CustomToolSchema]) -> Result<String> {
    #[derive(Serialize)]
    struct CustomToolDef<'a> {
        name: &'a str,
        description: &'a str,
        #[serde(rename = "inputSchema")]
        input_schema: &'a Map<String, Value>,
        max: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        output: &'a Option<String>,
    }

    let defs: Vec<_> = schemas
        .iter()
        .map(|schema| CustomToolDef {
            name: &schema.name,
            description: &schema.description,
            input_schema: &schema.input_schema,
            max: schema.max,
            output: &schema.output,
        })
        .collect();

    serde_json::to_string(&defs).context("failed to serialize custom tool schemas")
}

/// Serialize the fully resolved safe-output execution configuration consumed by
/// Stage 3 and custom-job preparation.
pub fn resolved_execution_config_json(
    front_matter: &FrontMatter,
    schemas: &[CustomToolSchema],
) -> Result<String> {
    let custom_tools: Value =
        serde_json::from_str(&custom_tools_json(schemas)?).context("invalid custom tools JSON")?;
    let mut tool_configs: Map<String, Value> = front_matter
        .safe_outputs
        .iter()
        .filter(|(key, _)| {
            !crate::compile::types::SAFE_OUTPUT_RESERVED_KEYS.contains(&key.as_str())
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    for tool in front_matter.all_safe_output_tool_names() {
        let config = tool_configs
            .entry(tool.clone())
            .or_insert_with(|| Value::Object(Map::new()));
        if !config.is_object() {
            *config = Value::Object(Map::new());
        }
        config
            .as_object_mut()
            .ok_or_else(|| anyhow!("failed to normalize resolved config for tool '{tool}'"))?
            .insert(
                "staged".to_string(),
                Value::Bool(front_matter.tool_is_staged(&tool)),
            );
    }
    let repositories: Vec<Value> = front_matter
        .repositories
        .iter()
        .map(|repository| {
            json!({
                "repository": repository.repository,
                "type": repository.repo_type,
                "name": repository.name,
                "ref": repository.repo_ref,
                "endpoint": repository.endpoint,
            })
        })
        .collect();
    let cache_memory = front_matter
        .tools
        .as_ref()
        .and_then(|tools| tools.cache_memory.as_ref())
        .map(|config| {
            json!({
                "enabled": config.is_enabled(),
                "allowedExtensions": config.allowed_extensions(),
            })
        });
    let debug_create_issue = front_matter
        .ado_aw_debug
        .as_ref()
        .and_then(|debug| debug.create_issue.as_ref())
        .map(serde_json::to_value)
        .transpose()
        .context("failed to serialize ado-aw-debug.create-issue")?;
    serde_json::to_string_pretty(&json!({
        "name": front_matter.name,
        "toolConfigs": tool_configs,
        "customTools": custom_tools,
        "repositories": repositories,
        "checkout": front_matter.checkout,
        "repoRefs": front_matter.checkout_repo_refs(),
        "cacheMemory": cache_memory,
        "debugCreateIssue": debug_create_issue,
    }))
    .context("failed to serialize resolved safe-output configuration")
}

/// Validate and default one custom-tool argument object against the generated
/// schema subset shared by MCP invocation and custom-job preparation.
pub fn validate_custom_arguments(
    tool_name: &str,
    schema: &Map<String, Value>,
    arguments: Option<Map<String, Value>>,
) -> Result<Map<String, Value>> {
    let mut arguments = arguments.unwrap_or_default();
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("custom tool '{tool_name}' schema is missing properties"))?;
    let required: HashSet<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();

    for key in arguments.keys() {
        if !properties.contains_key(key) {
            let safe_key = crate::sanitize::sanitize_config(key);
            bail!("custom tool '{tool_name}' argument '{safe_key}' is not allowed");
        }
    }

    for (name, property) in properties {
        let property = property.as_object().ok_or_else(|| {
            anyhow!("custom tool '{tool_name}' property '{name}' has an invalid schema")
        })?;
        if !arguments.contains_key(name)
            && let Some(default) = property.get("default")
        {
            arguments.insert(name.clone(), default.clone());
        }
        let Some(value) = arguments.get(name) else {
            ensure!(
                !required.contains(name.as_str()),
                "custom tool '{tool_name}' argument '{name}' is required"
            );
            continue;
        };

        match property.get("type").and_then(Value::as_str) {
            Some("string") => {
                let value = value.as_str().ok_or_else(|| {
                    anyhow!("custom tool '{tool_name}' argument '{name}' must be a string")
                })?;
                ensure!(
                    !required.contains(name.as_str()) || !value.trim().is_empty(),
                    "custom tool '{tool_name}' argument '{name}' is required"
                );
                ensure!(
                    value.len() <= CUSTOM_STRING_INPUT_MAX_BYTES,
                    "custom tool '{tool_name}' argument '{name}' exceeds the {} byte limit",
                    CUSTOM_STRING_INPUT_MAX_BYTES
                );
                if let Some(options) = property.get("enum").and_then(Value::as_array) {
                    ensure!(
                        options.iter().any(|option| option.as_str() == Some(value)),
                        "custom tool '{tool_name}' argument '{name}' must be one of: {}",
                        options
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
            }
            Some("boolean") => ensure!(
                value.is_boolean(),
                "custom tool '{tool_name}' argument '{name}' must be boolean"
            ),
            Some(other) => bail!(
                "custom tool '{tool_name}' property '{name}' has unsupported schema type '{other}'"
            ),
            None => bail!("custom tool '{tool_name}' property '{name}' is missing its type"),
        }
    }

    Ok(arguments)
}

fn validate_tool_name(tool_name: &str) -> Result<()> {
    ensure!(
        crate::validate::is_safe_tool_name(tool_name),
        "safe-outputs.jobs.{tool_name}: invalid custom tool name \
         (must be ASCII alphanumeric/hyphens only)"
    );
    ensure!(
        !crate::safe_outputs::ALL_KNOWN_SAFE_OUTPUTS.contains(&tool_name),
        "safe-outputs.jobs.{tool_name}: custom tool name collides with a built-in \
         safe-output tool"
    );
    ensure!(
        !matches!(tool_name, "scripts" | "jobs" | "require-approval" | "staged")
            && !CUSTOM_JOB_SYSTEM_NEEDS.contains(&tool_name),
        "safe-outputs.jobs.{tool_name}: custom tool name is reserved"
    );
    Ok(())
}

fn validate_input_name(tool_name: &str, input_name: &str) -> Result<()> {
    ensure!(
        crate::validate::is_valid_parameter_name(input_name),
        "safe-outputs.jobs.{tool_name}.inputs.{input_name}: invalid input name \
         (must match [A-Za-z_][A-Za-z0-9_]*)"
    );
    ensure!(
        !COMPILER_INPUT_KEYS.contains(&input_name),
        "safe-outputs.jobs.{tool_name}.inputs.{input_name}: input name is compiler-owned"
    );
    Ok(())
}

fn build_input_schema(tool_name: &str, inputs: Option<&Value>) -> Result<Map<String, Value>> {
    let mut schema = Map::new();
    schema.insert("type".to_string(), Value::String("object".to_string()));
    schema.insert("additionalProperties".to_string(), Value::Bool(false));

    let mut required = Vec::new();
    let mut properties = Map::new();

    if let Some(inputs_value) = inputs {
        let inputs_obj = inputs_value
            .as_object()
            .ok_or_else(|| anyhow!("safe-outputs.jobs.{tool_name}.inputs must be a mapping"))?;
        let mut names: Vec<&String> = inputs_obj.keys().collect();
        names.sort();

        for input_name in names {
            validate_input_name(tool_name, input_name)?;
            let path = format!("safe-outputs.jobs.{tool_name}.inputs.{input_name}");
            let input_obj = inputs_obj[input_name]
                .as_object()
                .ok_or_else(|| anyhow!("{path} must be a mapping"))?;
            reject_unknown_keys(input_obj, INPUT_KEYS, &path)?;

            if required_flag(input_obj, &path)? {
                required.push(Value::String(input_name.clone()));
            }
            properties.insert(
                input_name.clone(),
                input_schema(tool_name, input_name, input_obj)?,
            );
        }
    }

    schema.insert("required".to_string(), Value::Array(required));
    schema.insert("properties".to_string(), Value::Object(properties));
    Ok(schema)
}

fn input_schema(
    tool_name: &str,
    input_name: &str,
    input_obj: &Map<String, Value>,
) -> Result<Value> {
    let path = format!("safe-outputs.jobs.{tool_name}.inputs.{input_name}");
    let input_type = input_obj
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("{path}.type is required"))?;
    let description =
        required_nonempty_string(input_obj, "description", &format!("{path}.description"))?;

    let mut property = match input_type {
        "string" => json!({ "type": "string" }),
        "boolean" => json!({ "type": "boolean" }),
        "choice" => json!({
            "type": "string",
            "enum": choice_options(input_obj, &path)?,
        }),
        "number" => bail!(
            "{path}.type: custom jobs support string, boolean, or choice; number is not supported"
        ),
        other => bail!(
            "{path}.type has unsupported type '{other}' (expected string, boolean, or choice)"
        ),
    };
    let property_obj = property
        .as_object_mut()
        .ok_or_else(|| anyhow!("failed to construct object schema for {path}"))?;
    property_obj.insert("description".to_string(), Value::String(description));

    if let Some(default) = input_obj.get("default") {
        validate_default(input_type, default, input_obj, &path)?;
        property_obj.insert("default".to_string(), default.clone());
    }

    Ok(property)
}

fn validate_default(
    input_type: &str,
    default: &Value,
    input_obj: &Map<String, Value>,
    path: &str,
) -> Result<()> {
    match input_type {
        "string" => ensure!(default.is_string(), "{path}.default must be a string"),
        "boolean" => ensure!(default.is_boolean(), "{path}.default must be boolean"),
        "choice" => {
            let value = default
                .as_str()
                .ok_or_else(|| anyhow!("{path}.default must be a choice string"))?;
            let options = choice_options(input_obj, path)?;
            ensure!(
                options.iter().any(|option| option == value),
                "{path}.default must be one of: {}",
                options.join(", ")
            );
        }
        _ => unreachable!("input type validated before default"),
    }
    if let Some(value) = default.as_str() {
        ensure_agent_visible_literal(value, &format!("{path}.default"))?;
    }
    Ok(())
}

fn choice_options(input_obj: &Map<String, Value>, path: &str) -> Result<Vec<String>> {
    let options = input_obj
        .get("options")
        .ok_or_else(|| anyhow!("{path}.options is required"))?
        .as_array()
        .ok_or_else(|| anyhow!("{path}.options must be a list"))?;
    ensure!(!options.is_empty(), "{path}.options must not be empty");

    let mut seen = HashSet::new();
    options
        .iter()
        .map(|option| {
            let option = option
                .as_str()
                .ok_or_else(|| anyhow!("{path}.options entries must be strings"))?;
            ensure_agent_visible_literal(option, &format!("{path}.options"))?;
            ensure!(
                seen.insert(option.to_string()),
                "{path}.options contains duplicate value '{option}'"
            );
            Ok(option.to_string())
        })
        .collect()
}

fn required_flag(input_obj: &Map<String, Value>, path: &str) -> Result<bool> {
    match input_obj.get("required") {
        None => Ok(false),
        Some(Value::Bool(required)) => Ok(*required),
        Some(_) => bail!("{path}.required must be boolean"),
    }
}

fn parse_max(tool_name: &str, value: Option<&Value>) -> Result<usize> {
    let Some(value) = value else {
        return Ok(DEFAULT_CUSTOM_MAX);
    };
    let max = value
        .as_u64()
        .ok_or_else(|| anyhow!("safe-outputs.jobs.{tool_name}.max must be a positive integer"))?;
    ensure!(
        max > 0,
        "safe-outputs.jobs.{tool_name}.max must be a positive integer"
    );
    usize::try_from(max).context("custom safe-output max is too large")
}

fn parse_timeout(value: Option<&Value>, tool_name: &str) -> Result<Option<u32>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let timeout = value.as_u64().ok_or_else(|| {
        anyhow!("safe-outputs.jobs.{tool_name}.timeout-minutes must be a positive integer")
    })?;
    ensure!(
        timeout > 0,
        "safe-outputs.jobs.{tool_name}.timeout-minutes must be a positive integer"
    );
    Ok(Some(u32::try_from(timeout).with_context(|| {
        format!("safe-outputs.jobs.{tool_name}.timeout-minutes is too large")
    })?))
}

fn parse_needs(value: Option<&Value>, tool_name: &str) -> Result<Vec<String>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let values: Vec<&Value> = match value {
        Value::String(_) => vec![value],
        Value::Array(values) => values.iter().collect(),
        _ => bail!("safe-outputs.jobs.{tool_name}.needs must be a job name or list of job names"),
    };

    let mut needs = Vec::with_capacity(values.len());
    let mut seen = HashSet::new();
    for value in values {
        let need = value.as_str().ok_or_else(|| {
            anyhow!("safe-outputs.jobs.{tool_name}.needs entries must be strings")
        })?;
        ensure!(
            crate::validate::is_safe_tool_name(need),
            "safe-outputs.jobs.{tool_name}.needs entry '{need}' is not a safe job identifier"
        );
        if seen.insert(need.to_string()) {
            needs.push(need.to_string());
        }
    }
    Ok(needs)
}

fn parse_env(tool_name: &str, value: Option<&Value>) -> Result<Vec<(String, String)>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let env = value
        .as_object()
        .ok_or_else(|| anyhow!("safe-outputs.jobs.{tool_name}.env must be a mapping"))?;
    let mut pairs = Vec::with_capacity(env.len());
    for (name, value) in env {
        ensure!(
            crate::validate::is_valid_env_var_name(name),
            "safe-outputs.jobs.{tool_name}.env key '{name}' is not a valid environment variable name"
        );
        ensure!(
            !COMPILER_ENV_KEYS.contains(&name.as_str()),
            "safe-outputs.jobs.{tool_name}.env key '{name}' is compiler-owned"
        );
        let value = value
            .as_str()
            .ok_or_else(|| anyhow!("safe-outputs.jobs.{tool_name}.env.{name} must be a string"))?;
        ensure!(
            !crate::validate::contains_pipeline_command(value),
            "safe-outputs.jobs.{tool_name}.env.{name} must not contain an ADO pipeline command"
        );
        pairs.push((name.clone(), value.to_string()));
    }
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(pairs)
}

fn parse_component(
    tool_obj: &Map<String, Value>,
    tool_name: &str,
) -> Result<Option<CustomComponentDefinition>> {
    let source = tool_obj.get("component-source").and_then(Value::as_str);
    let sha = tool_obj.get("component-sha").and_then(Value::as_str);
    let has_provenance = source.is_some()
        || sha.is_some()
        || COMPONENT_PROVENANCE_KEYS
            .iter()
            .any(|key| tool_obj.contains_key(*key));
    if !has_provenance {
        return Ok(None);
    }

    let source = source.ok_or_else(|| {
        anyhow!(
            "safe-outputs.jobs.{tool_name} has incomplete component provenance: \
             component-source and component-sha must be present together"
        )
    })?;
    let sha = sha.ok_or_else(|| {
        anyhow!(
            "safe-outputs.jobs.{tool_name} has incomplete component provenance: \
             component-source and component-sha must be present together"
        )
    })?;
    let manifest_digest = match tool_obj.get("manifest-digest") {
        None | Some(Value::Null) => None,
        Some(Value::String(digest)) => {
            ensure!(
                digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "safe-outputs.jobs.{tool_name}.manifest-digest must be a 64-character \
                 hexadecimal SHA-256 digest"
            );
            Some(digest.to_ascii_lowercase())
        }
        Some(_) => {
            bail!("safe-outputs.jobs.{tool_name}.manifest-digest must be a string")
        }
    };

    Ok(Some(CustomComponentDefinition {
        source: source.to_string(),
        requested_ref: tool_obj
            .get("component-ref")
            .and_then(Value::as_str)
            .map(str::to_string),
        sha: CommitSha::parse(sha)
            .with_context(|| format!("safe-outputs.jobs.{tool_name}.component-sha"))?,
        manifest_digest,
    }))
}

fn reject_unknown_keys(obj: &Map<String, Value>, allowed: &[&str], path: &str) -> Result<()> {
    for key in obj.keys() {
        ensure!(
            allowed.contains(&key.as_str()),
            "{path} contains unsupported field '{key}'"
        );
    }
    Ok(())
}

fn required_nonempty_string(obj: &Map<String, Value>, key: &str, path: &str) -> Result<String> {
    let value = obj
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("{path} is required and must be a string"))?;
    ensure!(!value.trim().is_empty(), "{path} must not be empty");
    ensure_agent_visible_literal(value, path)?;
    Ok(value.to_string())
}

fn ensure_agent_visible_literal(value: &str, path: &str) -> Result<()> {
    ensure!(
        !crate::validate::contains_ado_expression(value),
        "{path} must not contain an ADO expression"
    );
    ensure!(
        !crate::validate::contains_pipeline_command(value),
        "{path} must not contain an ADO pipeline command"
    );
    Ok(())
}

fn optional_string(obj: &Map<String, Value>, key: &str) -> Result<Option<String>> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => bail!("must be a string when present"),
    }
}

fn ado_identifier_suffix(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
    {
        out
    } else {
        format!("_{out}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_front_matter(yaml: &str) -> FrontMatter {
        serde_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn jobs_generate_closed_gh_aw_compatible_schemas() {
        let fm = parse_front_matter(
            r#"
name: Test
description: Test
safe-outputs:
  jobs:
    send-notification:
      display-name: Send notification
      description: Send a structured notification.
      max: 2
      timeout-minutes: 10
      condition: succeeded()
      needs: [prepare]
      output: Notification proposal accepted.
      inputs:
        title:
          type: string
          description: Notification title.
          required: true
        urgent:
          type: boolean
          description: Whether the message is urgent.
          default: false
        severity:
          type: choice
          description: Operational severity.
          options: [info, warning, critical]
          default: info
      env:
        DESTINATION: release-operations
        TOKEN: $(SHARED_TOKEN)
      steps:
        - bash: echo ok
"#,
        );

        let definitions = collect_custom_tool_definitions(&fm).unwrap();
        assert_eq!(definitions.len(), 1);
        let definition = &definitions[0];
        assert_eq!(definition.max, 2);
        assert_eq!(definition.timeout_minutes, Some(10));
        assert_eq!(definition.needs, ["prepare"]);
        assert_eq!(
            definition.output.as_deref(),
            Some("Notification proposal accepted.")
        );
        assert_eq!(definition.input_schema["additionalProperties"], false);
        assert_eq!(
            definition.input_schema["properties"]["urgent"]["default"],
            false
        );
        assert_eq!(
            definition.input_schema["properties"]["severity"]["enum"],
            json!(["info", "warning", "critical"])
        );
        assert!(
            definition.input_schema["properties"]["title"]
                .get("maxLength")
                .is_none()
        );
    }

    #[test]
    fn scripts_are_rejected_with_jobs_guidance() {
        let fm = parse_front_matter(
            r#"
name: Test
description: Test
safe-outputs:
  scripts:
    notify:
      run: ./notify
"#,
        );
        let err = collect_custom_tool_definitions(&fm).unwrap_err();
        assert!(err.to_string().contains("safe-outputs.jobs"), "{err:#}");
    }

    #[test]
    fn max_defaults_to_one() {
        let fm = parse_front_matter(
            r#"
name: Test
description: Test
safe-outputs:
  jobs:
    notify:
      description: Notify.
      steps:
        - bash: echo ok
"#,
        );
        assert_eq!(
            collect_custom_tool_definitions(&fm).unwrap()[0].max,
            DEFAULT_CUSTOM_MAX
        );
    }

    #[test]
    fn number_and_max_length_are_rejected() {
        for input in [
            "type: number\n          description: Count.",
            "type: string\n          description: Text.\n          max-length: 10",
        ] {
            let fm = parse_front_matter(&format!(
                r#"
name: Test
description: Test
safe-outputs:
  jobs:
    notify:
      description: Notify.
      inputs:
        value:
          {input}
      steps:
        - bash: echo ok
"#
            ));
            assert!(collect_custom_tool_definitions(&fm).is_err());
        }
    }

    #[test]
    fn compiler_owned_input_and_env_names_are_rejected() {
        for input_name in ["name", "type"] {
            let fm = parse_front_matter(&format!(
                r#"
name: Test
description: Test
safe-outputs:
  jobs:
    notify:
      description: Notify.
      inputs:
        {input_name}:
          type: string
          description: Forged type.
      steps:
        - bash: echo ok
"#
            ));
            assert!(
                collect_custom_tool_definitions(&fm)
                    .unwrap_err()
                    .to_string()
                    .contains("compiler-owned")
            );
        }

        let fm = parse_front_matter(
            r#"
name: Test
description: Test
safe-outputs:
  jobs:
    notify:
      description: Notify.
      env:
        ADO_AW_AGENT_OUTPUT: forged
      steps:
        - bash: echo ok
"#,
        );
        assert!(
            collect_custom_tool_definitions(&fm)
                .unwrap_err()
                .to_string()
                .contains("compiler-owned")
        );
    }

    #[test]
    fn custom_env_values_reject_pipeline_commands_but_allow_ado_macros() {
        let rejected = parse_front_matter(
            r###"
name: Test
description: Test
safe-outputs:
  jobs:
    notify:
      description: Notify.
      env:
        MESSAGE: "##vso[task.setvariable variable=forged]value"
      steps:
        - bash: echo ok
"###,
        );
        assert!(
            collect_custom_tool_definitions(&rejected)
                .unwrap_err()
                .to_string()
                .contains("pipeline command")
        );

        let accepted = parse_front_matter(
            r#"
name: Test
description: Test
safe-outputs:
  jobs:
    notify:
      description: Notify.
      env:
        TOKEN: $(SHARED_TOKEN)
      steps:
        - bash: echo ok
"#,
        );
        assert_eq!(
            collect_custom_tool_definitions(&accepted).unwrap()[0].env,
            vec![("TOKEN".to_string(), "$(SHARED_TOKEN)".to_string())]
        );
    }

    #[test]
    fn component_manifest_digest_requires_sha256_hex() {
        let fm = parse_front_matter(
            r#"
name: Test
description: Test
safe-outputs:
  jobs:
    notify:
      description: Notify.
      component-source: org/repo/components/notify.md
      component-sha: 0123456789abcdef0123456789abcdef01234567
      manifest-digest: not-a-sha256
      steps:
        - bash: echo ok
"#,
        );
        let error = collect_custom_tool_definitions(&fm).unwrap_err();
        assert!(
            error.to_string().contains("64-character hexadecimal"),
            "{error:#}"
        );
    }

    #[test]
    fn system_job_names_are_reserved_for_custom_tools() {
        for name in CUSTOM_JOB_SYSTEM_NEEDS {
            assert!(
                validate_tool_name(name)
                    .unwrap_err()
                    .to_string()
                    .contains("reserved"),
                "{name}"
            );
        }
    }

    #[test]
    fn built_in_tool_name_collisions_are_rejected() {
        let fm = parse_front_matter(
            r#"
name: Test
description: Test
safe-outputs:
  jobs:
    create-work-item:
      description: Collides.
      steps:
        - bash: echo ok
"#,
        );
        assert!(
            collect_custom_tool_definitions(&fm)
                .unwrap_err()
                .to_string()
                .contains("collides with a built-in")
        );
    }

    #[test]
    fn custom_tools_json_includes_budget_and_acknowledgement() {
        let fm = parse_front_matter(
            r#"
name: Test
description: Test
safe-outputs:
  jobs:
    notify:
      description: Notify.
      max: 2
      output: Accepted.
      steps:
        - bash: echo ok
"#,
        );
        let json: Value = serde_json::from_str(
            &custom_tools_json(&generate_custom_tool_schemas(&fm).unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(json[0]["max"], 2);
        assert_eq!(json[0]["output"], "Accepted.");
        assert_eq!(json[0]["inputSchema"]["additionalProperties"], false);
    }

    #[test]
    fn resolved_execution_config_folds_global_staged_policy() {
        let fm = parse_front_matter(
            r#"
name: Test
description: Test
safe-outputs:
  staged: true
  noop: {}
  jobs:
    notify:
      description: Notify.
      steps:
        - bash: echo ok
"#,
        );
        let schemas = generate_custom_tool_schemas(&fm).unwrap();
        let config: Value =
            serde_json::from_str(&resolved_execution_config_json(&fm, &schemas).unwrap()).unwrap();
        assert_eq!(config["toolConfigs"]["noop"]["staged"], true);
        assert_eq!(config["toolConfigs"]["notify"]["staged"], true);
        assert_eq!(config["customTools"][0]["name"], "notify");
    }

    #[test]
    fn no_jobs_returns_empty_vec() {
        let fm = parse_front_matter("name: Test\ndescription: Test\n");
        assert!(generate_custom_tool_schemas(&fm).unwrap().is_empty());
    }
}
