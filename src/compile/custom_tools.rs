//! Compile-time schema generation for config-driven custom safe-output tools.

use std::collections::HashSet;

use anyhow::{Context, Result, anyhow, bail, ensure};
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::compile::types::FrontMatter;
use crate::secure::CommitSha;

const CUSTOM_TOOL_LIMIT: usize = 10;
const DEFAULT_STRING_MAX_LENGTH: u64 = 4_000;
const HARD_STRING_MAX_LENGTH: u64 = 8_000;
pub const DEFAULT_CUSTOM_MAX: usize = 3;
pub const DEFAULT_CUSTOM_SCRIPT_TIMEOUT_MINUTES: u32 = 10;
pub const MAX_CUSTOM_SCRIPT_TIMEOUT_MINUTES: u32 = 60;

pub const COMPONENT_PROVENANCE_KEYS: [&str; 5] = [
    "component-source",
    "component-sha",
    "manifest-digest",
    "component-repo-type",
    "component-endpoint",
];

/// A compiler-generated custom MCP tool definition.
#[derive(Debug, Clone, PartialEq)]
pub struct CustomToolSchema {
    pub name: String,
    pub description: String,
    pub input_schema: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CustomToolKind {
    Scripts {
        entrypoint: String,
        timeout_minutes: u32,
    },
    Jobs {
        steps: Vec<Value>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomComponentDefinition {
    pub alias: String,
    pub checkout_dir: String,
    pub source: String,
    pub sha: CommitSha,
    pub manifest_digest: Option<String>,
    pub repo_type: String,
    pub endpoint: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CustomToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Map<String, Value>,
    pub schema_digest: String,
    pub max: usize,
    pub env: Vec<(String, String)>,
    pub kind: CustomToolKind,
    pub component: Option<CustomComponentDefinition>,
}

pub fn collect_custom_tool_definitions(
    front_matter: &FrontMatter,
) -> Result<Vec<CustomToolDefinition>> {
    let mut definitions = Vec::new();
    let mut seen = HashSet::new();

    for section in ["scripts", "jobs"] {
        let Some(section_value) = front_matter.safe_outputs.get(section) else {
            continue;
        };
        let section_obj = section_value.as_object().ok_or_else(|| {
            anyhow!("safe-outputs.{section} must be a mapping of tool name to tool definition")
        })?;
        let mut names: Vec<&String> = section_obj.keys().collect();
        names.sort();

        for tool_name in names {
            let tool_def = section_obj
                .get(tool_name)
                .expect("tool name collected from map keys");
            validate_tool_name(section, tool_name)?;
            ensure!(
                seen.insert(tool_name.clone()),
                "custom safe-output tool '{tool_name}' is declared more than once"
            );

            let tool_obj = tool_def.as_object().ok_or_else(|| {
                anyhow!(
                    "safe-outputs.{section}.{tool_name} must be a mapping with optional \
                     description/max/executor fields and inputs"
                )
            })?;
            let description = optional_string(tool_obj, "description")
                .with_context(|| format!("safe-outputs.{section}.{tool_name}.description"))?
                .unwrap_or_default();
            let input_schema = build_input_schema(section, tool_name, tool_obj.get("inputs"))?;
            let schema_digest = crate::hash::sha256_hex(
                &serde_json::to_vec(&input_schema)
                    .context("failed to serialize custom tool schema for digest")?,
            );
            let max = parse_max(section, tool_name, tool_obj.get("max"))?;
            let env = parse_env(section, tool_name, tool_obj.get("env"))?;
            let component = parse_component(tool_obj, section, tool_name)?;
            let kind = if section == "scripts" {
                let entrypoint = tool_obj
                    .get("entrypoint")
                    .or_else(|| tool_obj.get("run"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        anyhow!("safe-outputs.scripts.{tool_name} requires `run` or `entrypoint`")
                    })?
                    .to_string();
                CustomToolKind::Scripts {
                    entrypoint,
                    timeout_minutes: parse_script_timeout(
                        section,
                        tool_name,
                        tool_obj.get("timeout-minutes"),
                    )?,
                }
            } else {
                ensure!(
                    !tool_obj.contains_key("timeout-minutes"),
                    "safe-outputs.jobs.{tool_name}.timeout-minutes is not supported; \
                     set timeouts on the authored ADO steps or job instead"
                );
                let steps = tool_obj
                    .get("steps")
                    .and_then(Value::as_array)
                    .ok_or_else(|| anyhow!("safe-outputs.jobs.{tool_name}.steps must be a list"))?
                    .clone();
                CustomToolKind::Jobs { steps }
            };

            definitions.push(CustomToolDefinition {
                name: tool_name.clone(),
                description,
                input_schema,
                schema_digest,
                max,
                env,
                kind,
                component,
            });
            ensure!(
                definitions.len() <= CUSTOM_TOOL_LIMIT,
                "custom safe-output tools per workflow must be <= {CUSTOM_TOOL_LIMIT}"
            );
        }
    }

    Ok(definitions)
}

/// Reject compiler-owned component provenance in the consumer's authored front
/// matter before imports are resolved. Remote-import provenance is stamped
/// later by the merge pass; accepting these fields here would let an ordinary
/// workflow forge a repository checkout and audit identity.
pub fn reject_author_component_provenance(front_matter: &FrontMatter) -> Result<()> {
    for section in ["scripts", "jobs"] {
        let Some(section_value) = front_matter.safe_outputs.get(section) else {
            continue;
        };
        let Some(tools) = section_value.as_object() else {
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

/// Generate closed JSON Schemas for custom tools under
/// `safe-outputs.scripts` and `safe-outputs.jobs`.
pub fn generate_custom_tool_schemas(front_matter: &FrontMatter) -> Result<Vec<CustomToolSchema>> {
    collect_custom_tool_definitions(front_matter).map(|definitions| {
        definitions
            .into_iter()
            .map(|definition| CustomToolSchema {
                name: definition.name,
                description: definition.description,
                input_schema: definition.input_schema,
            })
            .collect()
    })
}

/// Serialize schemas to the JSON array shape consumed by the SafeOutputs MCP
/// server's `--custom-tools` loader:
/// `[{ "name": ..., "description": ..., "inputSchema": ... }]`.
pub fn custom_tools_json(schemas: &[CustomToolSchema]) -> Result<String> {
    #[derive(Serialize)]
    struct CustomToolDef<'a> {
        name: &'a str,
        description: &'a str,
        #[serde(rename = "inputSchema")]
        input_schema: &'a Map<String, Value>,
    }

    let defs: Vec<_> = schemas
        .iter()
        .map(|schema| CustomToolDef {
            name: &schema.name,
            description: &schema.description,
            input_schema: &schema.input_schema,
        })
        .collect();

    serde_json::to_string(&defs).context("failed to serialize custom tool schemas")
}

fn validate_tool_name(section: &str, tool_name: &str) -> Result<()> {
    ensure!(
        crate::validate::is_safe_tool_name(tool_name),
        "safe-outputs.{section}.{tool_name}: invalid custom tool name \
         (must be ASCII alphanumeric/hyphens only)"
    );
    ensure!(
        !crate::safe_outputs::ALL_KNOWN_SAFE_OUTPUTS.contains(&tool_name),
        "safe-outputs.{section}.{tool_name}: custom tool name collides with a built-in \
         safe-output tool"
    );
    ensure!(
        !matches!(tool_name, "scripts" | "jobs"),
        "safe-outputs.{section}.{tool_name}: custom tool name is reserved for a \
         safe-outputs structural section"
    );
    Ok(())
}

fn validate_input_name(section: &str, tool_name: &str, input_name: &str) -> Result<()> {
    ensure!(
        crate::validate::is_valid_parameter_name(input_name),
        "safe-outputs.{section}.{tool_name}.inputs.{input_name}: invalid input name \
         (must match [A-Za-z_][A-Za-z0-9_]*)"
    );
    Ok(())
}

fn build_input_schema(
    section: &str,
    tool_name: &str,
    inputs: Option<&Value>,
) -> Result<Map<String, Value>> {
    let mut schema = Map::new();
    schema.insert("type".to_string(), Value::String("object".to_string()));
    schema.insert("additionalProperties".to_string(), Value::Bool(false));

    let mut required = Vec::new();
    let mut properties = Map::new();

    if let Some(inputs_value) = inputs {
        let inputs_obj = inputs_value.as_object().ok_or_else(|| {
            anyhow!("safe-outputs.{section}.{tool_name}.inputs must be a mapping")
        })?;

        for (input_name, input_def) in inputs_obj {
            validate_input_name(section, tool_name, input_name)?;
            let input_obj = input_def.as_object().ok_or_else(|| {
                anyhow!("safe-outputs.{section}.{tool_name}.inputs.{input_name} must be a mapping")
            })?;

            if required_flag(input_obj, section, tool_name, input_name)? {
                required.push(Value::String(input_name.clone()));
            }
            properties.insert(
                input_name.clone(),
                scalar_schema(section, tool_name, input_name, input_obj)?,
            );
        }
    }

    schema.insert("required".to_string(), Value::Array(required));
    schema.insert("properties".to_string(), Value::Object(properties));
    Ok(schema)
}

fn scalar_schema(
    section: &str,
    tool_name: &str,
    input_name: &str,
    input_obj: &Map<String, Value>,
) -> Result<Value> {
    let input_type = input_obj
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            anyhow!("safe-outputs.{section}.{tool_name}.inputs.{input_name}.type is required")
        })?;

    match input_type {
        "string" => Ok(json!({
            "type": "string",
            "maxLength": string_max_length(input_obj, section, tool_name, input_name)?,
        })),
        "number" => Ok(json!({ "type": "number" })),
        "boolean" => Ok(json!({ "type": "boolean" })),
        "choice" => Ok(json!({
            "type": "string",
            "enum": choice_options(input_obj, section, tool_name, input_name)?,
        })),
        "array" | "object" => bail!(
            "safe-outputs.{section}.{tool_name}.inputs.{input_name}: agent-facing \
             custom tool inputs are scalar-only; type '{input_type}' is not supported \
             (use string, number, boolean, or choice)"
        ),
        other => bail!(
            "safe-outputs.{section}.{tool_name}.inputs.{input_name}: unknown input type \
             '{other}' (expected string, number, boolean, or choice)"
        ),
    }
}

fn string_max_length(
    input_obj: &Map<String, Value>,
    section: &str,
    tool_name: &str,
    input_name: &str,
) -> Result<u64> {
    let Some(value) = input_obj.get("max-length") else {
        return Ok(DEFAULT_STRING_MAX_LENGTH);
    };
    let max = value.as_u64().ok_or_else(|| {
        anyhow!(
            "safe-outputs.{section}.{tool_name}.inputs.{input_name}.max-length must be \
             a positive integer"
        )
    })?;
    ensure!(
        max > 0,
        "safe-outputs.{section}.{tool_name}.inputs.{input_name}.max-length must be > 0"
    );
    ensure!(
        max <= HARD_STRING_MAX_LENGTH,
        "safe-outputs.{section}.{tool_name}.inputs.{input_name}.max-length must be <= \
         {HARD_STRING_MAX_LENGTH}"
    );
    Ok(max)
}

fn choice_options(
    input_obj: &Map<String, Value>,
    section: &str,
    tool_name: &str,
    input_name: &str,
) -> Result<Vec<String>> {
    let options = input_obj.get("options").ok_or_else(|| {
        anyhow!("safe-outputs.{section}.{tool_name}.inputs.{input_name}.options is required")
    })?;
    let options = options.as_array().ok_or_else(|| {
        anyhow!("safe-outputs.{section}.{tool_name}.inputs.{input_name}.options must be a list")
    })?;
    ensure!(
        !options.is_empty(),
        "safe-outputs.{section}.{tool_name}.inputs.{input_name}.options must not be empty"
    );

    options
        .iter()
        .map(|option| {
            option.as_str().map(str::to_string).ok_or_else(|| {
                anyhow!(
                    "safe-outputs.{section}.{tool_name}.inputs.{input_name}.options entries \
                     must be strings"
                )
            })
        })
        .collect()
}

fn required_flag(
    input_obj: &Map<String, Value>,
    section: &str,
    tool_name: &str,
    input_name: &str,
) -> Result<bool> {
    match input_obj.get("required") {
        None => Ok(false),
        Some(Value::Bool(required)) => Ok(*required),
        Some(_) => {
            bail!("safe-outputs.{section}.{tool_name}.inputs.{input_name}.required must be boolean")
        }
    }
}

fn optional_string(obj: &Map<String, Value>, key: &str) -> Result<Option<String>> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => bail!("must be a string when present"),
    }
}

fn parse_max(section: &str, tool_name: &str, value: Option<&Value>) -> Result<usize> {
    let Some(value) = value else {
        return Ok(DEFAULT_CUSTOM_MAX);
    };
    let max = value.as_u64().ok_or_else(|| {
        anyhow!("safe-outputs.{section}.{tool_name}.max must be a positive integer")
    })?;
    ensure!(
        max > 0,
        "safe-outputs.{section}.{tool_name}.max must be a positive integer"
    );
    usize::try_from(max)
        .with_context(|| format!("safe-outputs.{section}.{tool_name}.max is too large"))
}

fn parse_script_timeout(section: &str, tool_name: &str, value: Option<&Value>) -> Result<u32> {
    let Some(value) = value else {
        return Ok(DEFAULT_CUSTOM_SCRIPT_TIMEOUT_MINUTES);
    };
    let timeout = value.as_u64().ok_or_else(|| {
        anyhow!(
            "safe-outputs.{section}.{tool_name}.timeout-minutes must be an integer from 1 to \
             {MAX_CUSTOM_SCRIPT_TIMEOUT_MINUTES}"
        )
    })?;
    ensure!(
        (1..=u64::from(MAX_CUSTOM_SCRIPT_TIMEOUT_MINUTES)).contains(&timeout),
        "safe-outputs.{section}.{tool_name}.timeout-minutes must be from 1 to \
         {MAX_CUSTOM_SCRIPT_TIMEOUT_MINUTES}"
    );
    Ok(timeout as u32)
}

fn parse_env(
    section: &str,
    tool_name: &str,
    value: Option<&Value>,
) -> Result<Vec<(String, String)>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let env = value
        .as_object()
        .ok_or_else(|| anyhow!("safe-outputs.{section}.{tool_name}.env must be a mapping"))?;
    let mut pairs = Vec::new();
    for (name, value) in env {
        ensure!(
            crate::validate::is_valid_env_var_name(name),
            "safe-outputs.{section}.{tool_name}.env key `{name}` is not a valid environment variable name"
        );
        let variable = value.as_str().ok_or_else(|| {
            anyhow!("safe-outputs.{section}.{tool_name}.env.{name} must name an ADO variable")
        })?;
        ensure!(
            crate::validate::is_valid_ado_variable_name(variable),
            "safe-outputs.{section}.{tool_name}.env.{name} must be a valid ADO variable name"
        );
        pairs.push((name.clone(), variable.to_string()));
    }
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(pairs)
}

fn parse_component(
    tool_obj: &Map<String, Value>,
    section: &str,
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
            "safe-outputs.{section}.{tool_name} has incomplete component provenance: \
             component-source and component-sha must be present together"
        )
    })?;
    let sha = sha.ok_or_else(|| {
        anyhow!(
            "safe-outputs.{section}.{tool_name} has incomplete component provenance: \
             component-source and component-sha must be present together"
        )
    })?;
    let sha = CommitSha::parse(sha)
        .with_context(|| format!("safe-outputs.{section}.{tool_name}.component-sha"))?;
    let mut parts = source.splitn(3, '/');
    let owner = parts.next().unwrap_or_default();
    let repo = parts.next().unwrap_or_default();
    let path = parts.next().unwrap_or_default();
    ensure!(
        !owner.is_empty() && !repo.is_empty() && !path.is_empty(),
        "safe-outputs.{section}.{tool_name}.component-source must be owner/repo/path"
    );
    let repo_type = tool_obj
        .get("component-repo-type")
        .and_then(Value::as_str)
        .unwrap_or("git");
    ensure!(
        matches!(repo_type, "git" | "github" | "githubenterprise"),
        "safe-outputs.{section}.{tool_name}.component-repo-type must be git, github, or githubenterprise"
    );
    let endpoint = tool_obj
        .get("component-endpoint")
        .and_then(Value::as_str)
        .map(str::to_string);
    let alias = super::imports::alias::component_alias_identifier(
        owner,
        repo,
        repo_type,
        endpoint.as_deref(),
    );
    Ok(Some(CustomComponentDefinition {
        checkout_dir: format!("$(Build.SourcesDirectory)/{alias}"),
        alias,
        source: source.to_string(),
        sha,
        manifest_digest: tool_obj
            .get("manifest-digest")
            .and_then(Value::as_str)
            .map(str::to_string),
        repo_type: repo_type.to_string(),
        endpoint,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    fn parse_front_matter(yaml: &str) -> FrontMatter {
        serde_yaml::from_str(yaml).unwrap()
    }

    fn schema_by_name<'a>(schemas: &'a [CustomToolSchema], name: &str) -> &'a CustomToolSchema {
        schemas.iter().find(|schema| schema.name == name).unwrap()
    }

    #[test]
    fn scripts_and_jobs_generate_closed_scalar_schemas() {
        let fm = parse_front_matter(
            r#"
name: Test
description: Test
safe-outputs:
  scripts:
    send-notification:
      description: Send a structured notification.
      max: 3
      run: node notify.js
      inputs:
        title: { type: string, required: true, max-length: 120 }
        severity: { type: choice, options: [info, warning, critical], required: true }
  jobs:
    deploy-thing:
      description: Deploy via ADO steps.
      steps: []
      inputs:
        target: { type: string, required: true }
"#,
        );

        let schemas = generate_custom_tool_schemas(&fm).unwrap();
        assert_eq!(schemas.len(), 2);

        let send = schema_by_name(&schemas, "send-notification");
        assert_eq!(send.description, "Send a structured notification.");
        assert_eq!(send.input_schema["type"], "object");
        assert_eq!(send.input_schema["additionalProperties"], false);
        assert_eq!(
            send.input_schema["properties"]["title"],
            json!({ "type": "string", "maxLength": 120 })
        );
        assert_eq!(
            send.input_schema["properties"]["severity"],
            json!({ "type": "string", "enum": ["info", "warning", "critical"] })
        );
        let required = send.input_schema["required"].as_array().unwrap();
        assert!(required.contains(&Value::String("title".to_string())));
        assert!(required.contains(&Value::String("severity".to_string())));

        let deploy = schema_by_name(&schemas, "deploy-thing");
        assert_eq!(
            deploy.input_schema["properties"]["target"],
            json!({ "type": "string", "maxLength": DEFAULT_STRING_MAX_LENGTH })
        );

        let definitions = collect_custom_tool_definitions(&fm).unwrap();
        let send = definitions
            .iter()
            .find(|definition| definition.name == "send-notification")
            .unwrap();
        assert_eq!(send.max, 3);
        assert!(matches!(
            send.kind,
            CustomToolKind::Scripts {
                timeout_minutes: DEFAULT_CUSTOM_SCRIPT_TIMEOUT_MINUTES,
                ..
            }
        ));
        let deploy = definitions
            .iter()
            .find(|definition| definition.name == "deploy-thing")
            .unwrap();
        assert_eq!(deploy.max, DEFAULT_CUSTOM_MAX);
    }

    #[test]
    fn script_timeout_is_bounded_and_jobs_reject_it() {
        for timeout in [0, 61] {
            let fm = parse_front_matter(&format!(
                r#"
name: Test
description: Test
safe-outputs:
  scripts:
    notify:
      run: ./notify
      timeout-minutes: {timeout}
"#
            ));
            let err = collect_custom_tool_definitions(&fm).unwrap_err();
            assert!(err.to_string().contains("timeout-minutes"), "{err:#}");
        }

        let fm = parse_front_matter(
            r#"
name: Test
description: Test
safe-outputs:
  scripts:
    notify:
      run: ./notify
      timeout-minutes: 60
  jobs:
    deploy:
      timeout-minutes: 10
      steps: []
"#,
        );
        let err = collect_custom_tool_definitions(&fm).unwrap_err();
        assert!(err.to_string().contains("not supported"), "{err:#}");
    }

    #[test]
    fn component_sha_is_typed_and_partial_provenance_fails_closed() {
        let fm = parse_front_matter(
            r#"
name: Test
description: Test
safe-outputs:
  scripts:
    notify:
      run: ./notify
      component-source: octo/tools/components/notify.md
      component-sha: not-a-full-sha
"#,
        );
        let err = collect_custom_tool_definitions(&fm).unwrap_err();
        assert!(err.to_string().contains("component-sha"), "{err:#}");

        let fm = parse_front_matter(
            r#"
name: Test
description: Test
safe-outputs:
  scripts:
    notify:
      run: ./notify
      component-source: octo/tools/components/notify.md
"#,
        );
        let err = collect_custom_tool_definitions(&fm).unwrap_err();
        assert!(err.to_string().contains("incomplete"), "{err:#}");

        let fm = parse_front_matter(
            r#"
name: Test
description: Test
safe-outputs:
  scripts:
    notify:
      run: ./notify
      source: attacker/repo/component.md
      sha: 0123456789abcdef0123456789abcdef01234567
"#,
        );
        let definitions = collect_custom_tool_definitions(&fm).unwrap();
        assert!(
            definitions[0].component.is_none(),
            "legacy source/sha fields must not synthesize component provenance"
        );
    }

    #[test]
    fn authored_component_provenance_is_rejected_before_import_merge() {
        for key in COMPONENT_PROVENANCE_KEYS {
            let fm = parse_front_matter(&format!(
                r#"
name: Test
description: Test
safe-outputs:
  scripts:
    notify:
      run: ./notify
      {key}: forged
"#
            ));
            let err = reject_author_component_provenance(&fm).unwrap_err();
            let message = err.to_string();
            assert!(message.contains("compiler-owned"), "{message}");
            assert!(message.contains(key), "{message}");
        }
    }

    #[test]
    fn array_and_object_agent_inputs_are_rejected() {
        for input_type in ["array", "object"] {
            let fm = parse_front_matter(&format!(
                r#"
name: Test
description: Test
safe-outputs:
  scripts:
    bad-tool:
      run: ./tool
      inputs:
        payload: {{ type: {input_type} }}
"#
            ));

            let err = generate_custom_tool_schemas(&fm).unwrap_err();
            assert!(err.to_string().contains("scalar-only"));
        }
    }

    #[test]
    fn built_in_tool_name_collisions_are_rejected() {
        let fm = parse_front_matter(
            r#"
name: Test
description: Test
safe-outputs:
  scripts:
    create-work-item:
      run: ./tool
      inputs: {}
"#,
        );

        let err = generate_custom_tool_schemas(&fm).unwrap_err();
        assert!(err.to_string().contains("collides with a built-in"));
    }

    #[test]
    fn structural_section_names_are_rejected_as_custom_tools() {
        for name in ["scripts", "jobs"] {
            let fm = parse_front_matter(&format!(
                r#"
name: Test
description: Test
safe-outputs:
  scripts:
    {name}:
      run: ./tool
"#
            ));
            let err = collect_custom_tool_definitions(&fm).unwrap_err();
            assert!(err.to_string().contains("reserved"), "{err:#}");
        }
    }

    #[test]
    fn more_than_ten_custom_tools_is_rejected() {
        let mut yaml = String::from("name: Test\ndescription: Test\nsafe-outputs:\n  scripts:\n");
        for i in 0..11 {
            yaml.push_str(&format!(
                "    tool-{i}:\n      run: ./tool\n      inputs: {{}}\n"
            ));
        }
        let fm = parse_front_matter(&yaml);

        let err = generate_custom_tool_schemas(&fm).unwrap_err();
        assert!(
            err.to_string()
                .contains("custom safe-output tools per workflow")
        );
    }

    #[test]
    fn custom_tools_json_uses_camel_case_input_schema() {
        #[derive(Deserialize)]
        struct MirrorCustomToolDef {
            name: String,
            description: String,
            #[serde(rename = "inputSchema")]
            input_schema: Map<String, Value>,
        }

        let fm = parse_front_matter(
            r#"
name: Test
description: Test
safe-outputs:
  scripts:
    send-notification:
      description: Send a structured notification.
      run: node notify.js
      inputs:
        title: { type: string, required: true }
"#,
        );
        let schemas = generate_custom_tool_schemas(&fm).unwrap();
        let json = custom_tools_json(&schemas).unwrap();

        assert!(json.contains("\"inputSchema\""));
        assert!(!json.contains("input_schema"));
        let defs: Vec<MirrorCustomToolDef> = serde_json::from_str(&json).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "send-notification");
        assert_eq!(defs[0].description, "Send a structured notification.");
        assert_eq!(defs[0].input_schema["additionalProperties"], false);
    }

    #[test]
    fn no_scripts_or_jobs_returns_empty_vec() {
        let fm = parse_front_matter(
            r#"
name: Test
description: Test
"#,
        );

        let schemas = generate_custom_tool_schemas(&fm).unwrap();
        assert!(schemas.is_empty());
    }
}
