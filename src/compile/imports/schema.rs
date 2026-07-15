//! `import-schema` modeling, consumer-`with` validation, and
//! `{{ ado.aw.import-inputs.<key> }}` substitution.
#![allow(dead_code)]

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use serde_json::{Map as JsonMap, Value as JsonValue};
use serde_yaml::{Mapping as YamlMapping, Value as YamlValue};

const IMPORT_SCHEMA_KEY: &str = "import-schema";
const PLACEHOLDER_PREFIX: &str = "ado.aw.import-inputs.";

#[derive(Debug, Clone, PartialEq)]
pub struct ImportSchema {
    pub fields: BTreeMap<String, SchemaField>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SchemaField {
    pub ty: SchemaType,
    pub required: bool,
    pub default: Option<JsonValue>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SchemaType {
    String,
    Number,
    Boolean,
    Choice(Vec<String>),
    Array(Box<SchemaType>),
    Object(BTreeMap<String, SchemaField>),
}

/// Parses a component's `import-schema:` front-matter block.
///
/// Components without `import-schema:` return an empty schema.
pub fn parse_import_schema(front_matter: &YamlValue) -> Result<ImportSchema> {
    let Some(schema_value) = mapping_get(front_matter, IMPORT_SCHEMA_KEY) else {
        return Ok(ImportSchema {
            fields: BTreeMap::new(),
        });
    };

    let schema_map = yaml_mapping(schema_value, IMPORT_SCHEMA_KEY)?;
    Ok(ImportSchema {
        fields: parse_fields(schema_map, IMPORT_SCHEMA_KEY, 0)?,
    })
}

/// Validates consumer-provided `with:` values against an import schema.
///
/// The returned map includes validated provided values plus defaults for absent
/// fields that define `default:`.
pub fn validate_with(
    schema: &ImportSchema,
    with: &JsonMap<String, JsonValue>,
) -> Result<JsonMap<String, JsonValue>> {
    validate_fields(&schema.fields, with, "")
}

/// Substitutes `{{ ado.aw.import-inputs.<key> }}` placeholders in text.
///
/// This is an ado-aw **compile-time** replacement in the `{{ ... }}` family
/// (like `{{ workspace }}`), deliberately NOT the ADO template-expression
/// delimiter `${{ ... }}`: our substituted output is embedded directly into
/// pipeline YAML, and ADO template-processes any `${{ ... }}` it sees there, so
/// reusing that delimiter would be a footgun. A `{{` immediately preceded by
/// `$` is therefore treated as an ADO `${{ ... }}` expression and passed
/// through verbatim.
///
/// Whitespace around the expression inside `{{ ... }}` is allowed. Dotted paths
/// (for example `config.apiKey`) access object sub-fields. Missing keys are
/// intentionally left unchanged so [`apply_import_inputs`]'s leftover-placeholder
/// guard can flag them.
pub fn substitute_inputs(text: &str, inputs: &JsonMap<String, JsonValue>) -> String {
    let mut output = String::with_capacity(text.len());
    let mut cursor = 0;

    while let Some(relative_start) = text[cursor..].find("{{") {
        let start = cursor + relative_start;
        output.push_str(&text[cursor..start]);

        // A `{{` immediately preceded by `$` is an ADO `${{ ... }}` template
        // expression, never one of our markers — leave it verbatim (the `$`
        // was already pushed as part of `text[cursor..start]`).
        let preceded_by_dollar = start > 0 && text.as_bytes()[start - 1] == b'$';

        let expression_start = start + 2;
        let Some(relative_end) = text[expression_start..].find("}}") else {
            output.push_str(&text[start..]);
            return output;
        };
        let expression_end = expression_start + relative_end;
        let original = &text[start..expression_end + 2];
        let expression = text[expression_start..expression_end].trim();

        if preceded_by_dollar {
            output.push_str(original);
            cursor = expression_end + 2;
            continue;
        }

        match expression.strip_prefix(PLACEHOLDER_PREFIX) {
            Some(path) if !path.is_empty() => match lookup_input_path(inputs, path) {
                Some(value) => output.push_str(&render_json_value(value)),
                None => output.push_str(original),
            },
            _ => output.push_str(original),
        }

        cursor = expression_end + 2;
    }

    output.push_str(&text[cursor..]);
    output
}

/// Scans `text` for an **unresolved** `{{ ado.aw.import-inputs.<path> }}`
/// placeholder and returns its expression (e.g. `ado.aw.import-inputs.missing`)
/// if one remains after substitution.
///
/// Unlike [`substitute_inputs`], this does NOT skip a `$`-preceded `{{`: an
/// author who mistakenly wrote the old `${{ ado.aw.import-inputs.x }}` form
/// must also be flagged, because ADO would otherwise template-process it in the
/// emitted YAML. Any surviving marker of our namespace is a compile-time error
/// (a body reference to an input not supplied by the consumer `with:` / absent
/// from the component `import-schema:`).
fn find_input_placeholder_leak(text: &str) -> Option<String> {
    let mut cursor = 0;
    while let Some(relative_start) = text[cursor..].find("{{") {
        let start = cursor + relative_start;
        let expression_start = start + 2;
        let relative_end = text[expression_start..].find("}}")?;
        let expression_end = expression_start + relative_end;
        let expression = text[expression_start..expression_end].trim();
        if let Some(path) = expression.strip_prefix(PLACEHOLDER_PREFIX)
            && !path.is_empty()
        {
            return Some(expression.to_string());
        }
        cursor = expression_end + 2;
    }
    None
}

/// Walks front-matter string scalars for an unresolved import-input placeholder.
fn find_front_matter_placeholder_leak(fm: &YamlValue) -> Option<String> {
    match fm {
        YamlValue::String(s) => find_input_placeholder_leak(s),
        YamlValue::Sequence(items) => {
            items.iter().find_map(find_front_matter_placeholder_leak)
        }
        YamlValue::Mapping(mapping) => mapping.iter().find_map(|(key, value)| {
            find_front_matter_placeholder_leak(key)
                .or_else(|| find_front_matter_placeholder_leak(value))
        }),
        _ => None,
    }
}

/// Walks front matter and substitutes import-input placeholders in every string
/// scalar.
pub fn substitute_front_matter(fm: &YamlValue, inputs: &JsonMap<String, JsonValue>) -> YamlValue {
    match fm {
        YamlValue::String(s) => YamlValue::String(substitute_inputs(s, inputs)),
        YamlValue::Sequence(items) => YamlValue::Sequence(
            items
                .iter()
                .map(|item| substitute_front_matter(item, inputs))
                .collect(),
        ),
        YamlValue::Mapping(mapping) => {
            let mut substituted = YamlMapping::new();
            for (key, value) in mapping {
                substituted.insert(
                    substitute_front_matter(key, inputs),
                    substitute_front_matter(value, inputs),
                );
            }
            YamlValue::Mapping(substituted)
        }
        other => other.clone(),
    }
}

/// Parses, validates, defaults, substitutes, and consumes `import-schema:`.
///
/// This is intentionally a pure transformation: it does not mutate a
/// `ResolvedImport` and does not merge the component into the consumer
/// workflow.
pub fn apply_import_inputs(
    front_matter: &YamlValue,
    body: &str,
    with: &JsonMap<String, JsonValue>,
) -> Result<(YamlValue, String)> {
    let schema = parse_import_schema(front_matter)?;
    let inputs = validate_with(&schema, with)?;
    let stripped_front_matter = strip_import_schema(front_matter);

    let substituted_front_matter = substitute_front_matter(&stripped_front_matter, &inputs);
    let substituted_body = substitute_inputs(body, &inputs);

    // Leftover-placeholder guard: any `{{ ado.aw.import-inputs.<path> }}` still
    // present after substitution is a reference to an input the consumer did
    // not supply (via `with:`) and that the component `import-schema:` did not
    // default. Fail closed — an unresolved marker embedded into the pipeline
    // YAML or agent prompt is a footgun (ADO would template-process a stray
    // `${{ ... }}`, and a leaked prompt marker is meaningless to the agent).
    if let Some(expr) = find_front_matter_placeholder_leak(&substituted_front_matter)
        .or_else(|| find_input_placeholder_leak(&substituted_body))
    {
        let key = expr.strip_prefix(PLACEHOLDER_PREFIX).unwrap_or(&expr);
        anyhow::bail!(
            "unresolved import input placeholder `{{{{ {expr} }}}}`: no input named `{key}` \
             was provided in the consumer `with:` or defaulted by the component \
             `import-schema:` (note: use the compile-time `{{{{ ... }}}}` delimiter, not \
             the ADO `${{{{ ... }}}}` template-expression delimiter)"
        );
    }

    Ok((substituted_front_matter, substituted_body))
}

fn parse_fields(
    fields_map: &YamlMapping,
    path: &str,
    object_depth: usize,
) -> Result<BTreeMap<String, SchemaField>> {
    let mut fields = BTreeMap::new();
    for (key, value) in fields_map {
        let field_name = yaml_string(key, path)?;
        let field_path = dotted_path(path, field_name);
        if fields
            .insert(
                field_name.to_string(),
                parse_schema_field(value, &field_path, object_depth)?,
            )
            .is_some()
        {
            anyhow::bail!("duplicate import-schema field `{field_path}`");
        }
    }
    Ok(fields)
}

fn parse_schema_field(value: &YamlValue, path: &str, object_depth: usize) -> Result<SchemaField> {
    let field_map = yaml_mapping(value, path)?;
    let ty = parse_schema_type(field_map, path, object_depth)?;
    let required = match mapping_get_in(field_map, "required") {
        Some(YamlValue::Bool(required)) => *required,
        Some(_) => anyhow::bail!("import-schema field `{path}.required` must be a boolean"),
        None => false,
    };
    let default = mapping_get_in(field_map, "default")
        .map(|value| yaml_to_json(value, &dotted_path(path, "default")))
        .transpose()?;
    let description = match mapping_get_in(field_map, "description") {
        Some(YamlValue::String(description)) => Some(description.clone()),
        Some(_) => anyhow::bail!("import-schema field `{path}.description` must be a string"),
        None => None,
    };

    Ok(SchemaField {
        ty,
        required,
        default,
        description,
    })
}

fn parse_schema_type(
    field_map: &YamlMapping,
    path: &str,
    object_depth: usize,
) -> Result<SchemaType> {
    let ty_value = mapping_get_in(field_map, "type")
        .ok_or_else(|| anyhow::anyhow!("import-schema field `{path}` is missing `type`"))?;
    let ty = yaml_string(ty_value, &dotted_path(path, "type"))?;

    match ty {
        "string" => Ok(SchemaType::String),
        "number" => Ok(SchemaType::Number),
        "boolean" => Ok(SchemaType::Boolean),
        "choice" => parse_choice_type(field_map, path),
        "array" => parse_array_type(field_map, path, object_depth),
        "object" => parse_object_type(field_map, path, object_depth),
        other => anyhow::bail!("import-schema field `{path}.type` has unsupported type `{other}`"),
    }
}

fn parse_choice_type(field_map: &YamlMapping, path: &str) -> Result<SchemaType> {
    let options_value = mapping_get_in(field_map, "options").ok_or_else(|| {
        anyhow::anyhow!("choice import-schema field `{path}` is missing `options`")
    })?;
    let options_sequence = yaml_sequence(options_value, &dotted_path(path, "options"))?;
    let mut options = Vec::with_capacity(options_sequence.len());
    for (index, option) in options_sequence.iter().enumerate() {
        options.push(yaml_string(option, &format!("{}.options[{index}]", path))?.to_string());
    }
    Ok(SchemaType::Choice(options))
}

fn parse_array_type(
    field_map: &YamlMapping,
    path: &str,
    object_depth: usize,
) -> Result<SchemaType> {
    let items_value = mapping_get_in(field_map, "items")
        .ok_or_else(|| anyhow::anyhow!("array import-schema field `{path}` is missing `items`"))?;
    let items_map = yaml_mapping(items_value, &dotted_path(path, "items"))?;
    Ok(SchemaType::Array(Box::new(parse_schema_type(
        items_map,
        &dotted_path(path, "items"),
        object_depth,
    )?)))
}

fn parse_object_type(
    field_map: &YamlMapping,
    path: &str,
    object_depth: usize,
) -> Result<SchemaType> {
    if object_depth > 0 {
        anyhow::bail!(
            "nested object import-schema field `{path}` is not supported; object properties are one level deep"
        );
    }
    let properties_value = mapping_get_in(field_map, "properties").ok_or_else(|| {
        anyhow::anyhow!("object import-schema field `{path}` is missing `properties`")
    })?;
    let properties_map = yaml_mapping(properties_value, &dotted_path(path, "properties"))?;
    Ok(SchemaType::Object(parse_fields(
        properties_map,
        &dotted_path(path, "properties"),
        object_depth + 1,
    )?))
}

fn validate_fields(
    fields: &BTreeMap<String, SchemaField>,
    with: &JsonMap<String, JsonValue>,
    path_prefix: &str,
) -> Result<JsonMap<String, JsonValue>> {
    for key in with.keys() {
        if !fields.contains_key(key) {
            anyhow::bail!("unknown import input `{}`", dotted_path(path_prefix, key));
        }
    }

    let mut effective = JsonMap::new();
    for (name, field) in fields {
        let path = dotted_path(path_prefix, name);
        match with.get(name) {
            Some(value) => {
                effective.insert(name.clone(), validate_value(&field.ty, value, &path)?);
            }
            None if field.default.is_some() => {
                let default = field.default.as_ref().expect("checked is_some");
                effective.insert(name.clone(), validate_value(&field.ty, default, &path)?);
            }
            None if field.required => {
                anyhow::bail!("missing required import input `{path}`");
            }
            None => {}
        }
    }
    Ok(effective)
}

fn validate_value(ty: &SchemaType, value: &JsonValue, path: &str) -> Result<JsonValue> {
    match ty {
        SchemaType::String => match value {
            JsonValue::String(_) => Ok(value.clone()),
            _ => type_error(path, "string", value),
        },
        SchemaType::Number => {
            if value.is_number() {
                Ok(value.clone())
            } else {
                type_error(path, "number", value)
            }
        }
        SchemaType::Boolean => match value {
            JsonValue::Bool(_) => Ok(value.clone()),
            _ => type_error(path, "boolean", value),
        },
        SchemaType::Choice(options) => match value {
            JsonValue::String(value) if options.contains(value) => {
                Ok(JsonValue::String(value.clone()))
            }
            JsonValue::String(value) => anyhow::bail!(
                "import input `{path}` value `{value}` is not one of: {}",
                options.join(", ")
            ),
            _ => type_error(path, "choice string", value),
        },
        SchemaType::Array(item_ty) => match value {
            JsonValue::Array(items) => {
                let mut validated = Vec::with_capacity(items.len());
                for (index, item) in items.iter().enumerate() {
                    validated.push(validate_value(item_ty, item, &format!("{path}[{index}]"))?);
                }
                Ok(JsonValue::Array(validated))
            }
            _ => type_error(path, "array", value),
        },
        SchemaType::Object(properties) => match value {
            JsonValue::Object(object) => Ok(JsonValue::Object(validate_fields(
                properties, object, path,
            )?)),
            _ => type_error(path, "object", value),
        },
    }
}

fn type_error(path: &str, expected: &str, value: &JsonValue) -> Result<JsonValue> {
    anyhow::bail!(
        "import input `{path}` must be {expected}, got {}",
        json_value_kind(value)
    )
}

fn strip_import_schema(front_matter: &YamlValue) -> YamlValue {
    let YamlValue::Mapping(mapping) = front_matter else {
        return front_matter.clone();
    };

    let mut stripped = YamlMapping::new();
    for (key, value) in mapping {
        if matches!(key, YamlValue::String(key) if key == IMPORT_SCHEMA_KEY) {
            continue;
        }
        stripped.insert(key.clone(), value.clone());
    }
    YamlValue::Mapping(stripped)
}

fn mapping_get<'a>(value: &'a YamlValue, key: &str) -> Option<&'a YamlValue> {
    let YamlValue::Mapping(mapping) = value else {
        return None;
    };
    mapping_get_in(mapping, key)
}

fn mapping_get_in<'a>(mapping: &'a YamlMapping, key: &str) -> Option<&'a YamlValue> {
    mapping.iter().find_map(|(mapping_key, value)| {
        if matches!(mapping_key, YamlValue::String(mapping_key) if mapping_key == key) {
            Some(value)
        } else {
            None
        }
    })
}

fn yaml_mapping<'a>(value: &'a YamlValue, path: &str) -> Result<&'a YamlMapping> {
    match value {
        YamlValue::Mapping(mapping) => Ok(mapping),
        _ => anyhow::bail!(
            "import-schema field `{path}` must be a mapping, got {}",
            super::yaml_value_kind(value)
        ),
    }
}

fn yaml_sequence<'a>(value: &'a YamlValue, path: &str) -> Result<&'a Vec<YamlValue>> {
    match value {
        YamlValue::Sequence(sequence) => Ok(sequence),
        _ => anyhow::bail!(
            "import-schema field `{path}` must be a sequence, got {}",
            super::yaml_value_kind(value)
        ),
    }
}

fn yaml_string<'a>(value: &'a YamlValue, path: &str) -> Result<&'a str> {
    match value {
        YamlValue::String(value) => Ok(value),
        _ => anyhow::bail!(
            "import-schema field `{path}` must be a string, got {}",
            super::yaml_value_kind(value)
        ),
    }
}

fn yaml_to_json(value: &YamlValue, path: &str) -> Result<JsonValue> {
    serde_json::to_value(value)
        .with_context(|| format!("import-schema field `{path}` default is not JSON-compatible"))
}

fn lookup_input_path<'a>(
    inputs: &'a JsonMap<String, JsonValue>,
    path: &str,
) -> Option<&'a JsonValue> {
    let mut parts = path.split('.');
    let first = parts.next()?;
    if first.is_empty() {
        return None;
    }

    let mut value = inputs.get(first)?;
    for part in parts {
        if part.is_empty() {
            return None;
        }
        value = value.as_object()?.get(part)?;
    }
    Some(value)
}

/// Render an import-input value for interpolation into a `{{ ... }}`
/// placeholder.
///
/// **Trust boundary.** Import inputs are non-secret, compile-time author
/// choices: a component's schema defaults are pinned by commit SHA (reviewed at
/// import time) and a consumer's `with:` values are committed to the consumer
/// repo (reviewed at author time). Neither is agent- or runtime-controlled.
/// As defense-in-depth we still run string values through
/// [`crate::sanitize::sanitize_config`], which neutralizes Azure DevOps
/// pipeline logging commands (`##vso[` / `##[`) and strips control characters,
/// so an interpolated value can never smuggle a pipeline command into a
/// generated step. Non-string scalars/containers are rendered structurally and
/// need no neutralization.
fn render_json_value(value: &JsonValue) -> String {
    match value {
        JsonValue::String(value) => crate::sanitize::sanitize_config(value),
        JsonValue::Number(value) => value.to_string(),
        JsonValue::Bool(value) => value.to_string(),
        JsonValue::Array(_) | JsonValue::Object(_) => {
            serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
        }
        JsonValue::Null => "null".to_string(),
    }
}

fn dotted_path(prefix: &str, key: &str) -> String {
    if prefix.is_empty() {
        key.to_string()
    } else {
        format!("{prefix}.{key}")
    }
}

fn json_value_kind(value: &JsonValue) -> &'static str {
    match value {
        JsonValue::Null => "null",
        JsonValue::Bool(_) => "boolean",
        JsonValue::Number(_) => "number",
        JsonValue::String(_) => "string",
        JsonValue::Array(_) => "array",
        JsonValue::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn yaml(input: &str) -> YamlValue {
        serde_yaml::from_str(input).expect("valid yaml")
    }

    fn schema_yaml() -> YamlValue {
        yaml(
            r#"
import-schema:
  name:
    type: string
    required: true
    description: Component name
  count:
    type: number
    default: 3
  enabled:
    type: boolean
    default: true
  mode:
    type: choice
    options: [fast, slow]
  tags:
    type: array
    items:
      type: string
  config:
    type: object
    properties:
      apiKey:
        type: string
        required: true
      retries:
        type: number
        default: 2
"#,
        )
    }

    #[test]
    fn parse_import_schema_supports_all_types_required_default_and_description() {
        let schema = parse_import_schema(&schema_yaml()).unwrap();

        assert!(matches!(schema.fields["name"].ty, SchemaType::String));
        assert!(schema.fields["name"].required);
        assert_eq!(
            schema.fields["name"].description.as_deref(),
            Some("Component name")
        );
        assert!(matches!(schema.fields["count"].ty, SchemaType::Number));
        assert_eq!(schema.fields["count"].default, Some(json!(3)));
        assert!(matches!(schema.fields["enabled"].ty, SchemaType::Boolean));
        assert_eq!(schema.fields["enabled"].default, Some(json!(true)));
        assert_eq!(
            schema.fields["mode"].ty,
            SchemaType::Choice(vec!["fast".to_string(), "slow".to_string()])
        );
        assert_eq!(
            schema.fields["tags"].ty,
            SchemaType::Array(Box::new(SchemaType::String))
        );
        match &schema.fields["config"].ty {
            SchemaType::Object(properties) => {
                assert!(matches!(properties["apiKey"].ty, SchemaType::String));
                assert!(properties["apiKey"].required);
                assert_eq!(properties["retries"].default, Some(json!(2)));
            }
            other => panic!("expected object schema, got {other:?}"),
        }
    }

    #[test]
    fn parse_import_schema_returns_empty_when_missing() {
        let schema = parse_import_schema(&yaml("name: example\n")).unwrap();

        assert!(schema.fields.is_empty());
    }

    #[test]
    fn validate_with_fills_defaults_and_object_property_defaults() {
        let schema = parse_import_schema(&schema_yaml()).unwrap();
        let with = json!({
            "name": "demo",
            "mode": "fast",
            "tags": ["a", "b"],
            "config": { "apiKey": "secret" }
        });
        let validated = validate_with(&schema, with.as_object().unwrap()).unwrap();

        assert_eq!(validated["name"], json!("demo"));
        assert_eq!(validated["count"], json!(3));
        assert_eq!(validated["enabled"], json!(true));
        assert_eq!(validated["tags"], json!(["a", "b"]));
        assert_eq!(
            validated["config"],
            json!({ "apiKey": "secret", "retries": 2 })
        );
    }

    #[test]
    fn validate_with_errors_for_missing_required() {
        let schema = parse_import_schema(&schema_yaml()).unwrap();
        let err = validate_with(&schema, &JsonMap::new()).unwrap_err();

        assert!(
            err.to_string()
                .contains("missing required import input `name`")
        );
    }

    #[test]
    fn validate_with_errors_for_unknown_key() {
        let schema = parse_import_schema(&schema_yaml()).unwrap();
        let with = json!({ "name": "demo", "unknown": true });
        let err = validate_with(&schema, with.as_object().unwrap()).unwrap_err();

        assert!(err.to_string().contains("unknown import input `unknown`"));
    }

    #[test]
    fn validate_with_errors_for_choice_not_in_options() {
        let schema = parse_import_schema(&schema_yaml()).unwrap();
        let with = json!({ "name": "demo", "mode": "medium" });
        let err = validate_with(&schema, with.as_object().unwrap()).unwrap_err();

        assert!(err.to_string().contains("mode"));
        assert!(err.to_string().contains("fast, slow"));
    }

    #[test]
    fn validate_with_errors_for_array_element_type_mismatch() {
        let schema = parse_import_schema(&schema_yaml()).unwrap();
        let with = json!({ "name": "demo", "tags": ["ok", 1] });
        let err = validate_with(&schema, with.as_object().unwrap()).unwrap_err();

        assert!(err.to_string().contains("tags[1]"));
        assert!(err.to_string().contains("string"));
    }

    #[test]
    fn substitute_inputs_supports_scalars_dotted_paths_json_values_and_missing_passthrough() {
        let inputs = json!({
            "name": "demo",
            "count": 7,
            "enabled": false,
            "tags": ["a", "b"],
            "config": { "apiKey": "secret" }
        });
        let text = concat!(
            "name={{ado.aw.import-inputs.name}} ",
            "key={{ ado.aw.import-inputs.config.apiKey }} ",
            "count={{ ado.aw.import-inputs.count }} ",
            "enabled={{ ado.aw.import-inputs.enabled }} ",
            "tags={{ ado.aw.import-inputs.tags }} ",
            "config={{ ado.aw.import-inputs.config }} ",
            "missing={{ ado.aw.import-inputs.missing }}"
        );

        let substituted = substitute_inputs(text, inputs.as_object().unwrap());

        assert_eq!(
            substituted,
            concat!(
                "name=demo ",
                "key=secret ",
                "count=7 ",
                "enabled=false ",
                "tags=[\"a\",\"b\"] ",
                "config={\"apiKey\":\"secret\"} ",
                "missing={{ ado.aw.import-inputs.missing }}"
            )
        );
    }

    #[test]
    fn substitute_inputs_preserves_ado_template_expressions() {
        // A `$`-preceded `{{ ... }}` is an ADO `${{ ... }}` template expression,
        // NOT one of our markers — it must pass through untouched, even when it
        // syntactically mentions our namespace (that leak is caught later by the
        // guard, not silently substituted here).
        let inputs = json!({ "name": "demo" });
        let text = "keep ${{ parameters.env }} and {{ ado.aw.import-inputs.name }} and ${{ ado.aw.import-inputs.name }}";
        let substituted = substitute_inputs(text, inputs.as_object().unwrap());
        assert_eq!(
            substituted,
            "keep ${{ parameters.env }} and demo and ${{ ado.aw.import-inputs.name }}"
        );
    }

    #[test]
    fn substitute_inputs_preserves_runtime_import_markers() {
        let inputs = json!({ "name": "demo" });
        let text = "{{#runtime-import agents/x.md}} uses {{ ado.aw.import-inputs.name }}";
        let substituted = substitute_inputs(text, inputs.as_object().unwrap());
        assert_eq!(substituted, "{{#runtime-import agents/x.md}} uses demo");
    }

    #[test]
    fn substitute_inputs_neutralizes_pipeline_commands_in_string_values() {
        // A string import input containing an ADO logging command must be
        // neutralized when interpolated (defense-in-depth), so it cannot smuggle
        // a `##vso[` pipeline command into a generated step.
        let inputs = json!({ "evil": "##vso[task.setvariable variable=x]y" });
        let substituted = substitute_inputs("val={{ ado.aw.import-inputs.evil }}", inputs.as_object().unwrap());
        // The neutralized form wraps the command in backticks so ADO renders it
        // as inert text instead of executing it.
        assert!(
            substituted.contains("`##vso[`"),
            "expected neutralized (backtick-wrapped) form: {substituted}"
        );
        assert!(
            !substituted.contains("##vso[task.setvariable"),
            "the executable command tail must be broken up: {substituted}"
        );
    }

    #[test]
    fn substitute_front_matter_walks_nested_mappings_and_sequences() {
        let fm = yaml(
            r#"
name: "{{ ado.aw.import-inputs.name }}"
steps:
  - bash: echo {{ ado.aw.import-inputs.config.apiKey }}
nested:
  value: before {{ado.aw.import-inputs.name}} after
"#,
        );
        let inputs = json!({
            "name": "demo",
            "config": { "apiKey": "secret" }
        });

        let substituted = substitute_front_matter(&fm, inputs.as_object().unwrap());

        assert_eq!(mapping_get(&substituted, "name"), Some(&yaml("demo")));
        let steps = mapping_get(&substituted, "steps")
            .unwrap()
            .as_sequence()
            .unwrap();
        assert_eq!(mapping_get(&steps[0], "bash"), Some(&yaml("echo secret")));
        let nested = mapping_get(&substituted, "nested").unwrap();
        assert_eq!(
            mapping_get(nested, "value"),
            Some(&yaml("before demo after"))
        );
    }

    #[test]
    fn apply_import_inputs_strips_schema_and_substitutes_front_matter_and_body() {
        let fm = yaml(
            r#"
import-schema:
  name:
    type: string
    required: true
  count:
    type: number
    default: 2
name: component-{{ ado.aw.import-inputs.name }}
variables:
  count: "{{ ado.aw.import-inputs.count }}"
"#,
        );
        let with = json!({ "name": "demo" });

        let (front_matter, body) = apply_import_inputs(
            &fm,
            "Hello {{ ado.aw.import-inputs.name }} {{ ado.aw.import-inputs.count }}",
            with.as_object().unwrap(),
        )
        .unwrap();

        assert!(mapping_get(&front_matter, IMPORT_SCHEMA_KEY).is_none());
        assert_eq!(
            mapping_get(&front_matter, "name"),
            Some(&yaml("component-demo"))
        );
        let variables = mapping_get(&front_matter, "variables").unwrap();
        assert_eq!(
            mapping_get(variables, "count"),
            Some(&YamlValue::String("2".to_string()))
        );
        assert_eq!(body, "Hello demo 2");
    }

    #[test]
    fn apply_import_inputs_rejects_unresolved_body_placeholder() {
        // `typo` is not in the schema and not supplied — `validate_with` won't
        // catch a body-only reference, so the leftover guard must.
        let fm = yaml("import-schema:\n  name:\n    type: string\n");
        let with = json!({ "name": "demo" });
        let err = apply_import_inputs(
            &fm,
            "Hello {{ ado.aw.import-inputs.typo }}",
            with.as_object().unwrap(),
        )
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("unresolved import input placeholder"), "{msg}");
        assert!(msg.contains("ado.aw.import-inputs.typo"), "{msg}");
    }

    #[test]
    fn apply_import_inputs_rejects_leftover_dollar_delimited_marker() {
        // An author who mistakenly used the old `${{ ... }}` form is flagged:
        // it would otherwise be template-processed by ADO in the emitted YAML.
        let fm = yaml("import-schema:\n  name:\n    type: string\n");
        let with = json!({ "name": "demo" });
        let err = apply_import_inputs(
            &fm,
            "Hello ${{ ado.aw.import-inputs.name }}",
            with.as_object().unwrap(),
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("unresolved import input placeholder"),
            "{err:#}"
        );
    }

    #[test]
    fn apply_import_inputs_leaves_ado_template_expression_untouched() {
        // A genuine ADO `${{ parameters.x }}` (not our namespace) must survive
        // substitution and NOT trip the guard.
        let fm = yaml("name: keep\n");
        let with = json!({});
        let (_, body) = apply_import_inputs(
            &fm,
            "run in ${{ parameters.environment }}",
            with.as_object().unwrap(),
        )
        .unwrap();
        assert_eq!(body, "run in ${{ parameters.environment }}");
    }
}
