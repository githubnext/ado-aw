//! Serializable specification for the runtime prompt renderer
//! (`scripts/prompt.js`). Mirrors the design of `filter_ir::GateSpec`.
//!
//! The compiler builds a [`PromptSpec`] at compile time, JSON-serializes it,
//! base64-encodes the result, and emits it as `ADO_AW_PROMPT_SPEC` env on the
//! prompt.js step. At pipeline runtime, prompt.js decodes the spec, validates
//! the version, reads the source markdown from the workspace, strips its
//! front matter, applies variable substitution, appends supplements, and
//! writes the rendered prompt to `output_path`.

use schemars::JsonSchema;
use serde::Serialize;

/// Pinned schema version. Bump only on breaking changes to [`PromptSpec`].
/// `prompt.js` refuses to run on an unknown version.
pub const PROMPT_SPEC_VERSION: u32 = 1;

/// Top-level spec consumed by `prompt.js` at pipeline runtime.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PromptSpec {
    /// Schema version; refused on mismatch.
    pub version: u32,
    /// Absolute path to the source `.md` file in the workspace.
    pub source_path: String,
    /// Absolute path where the rendered prompt should be written.
    pub output_path: String,
    /// Extension prompt supplements, in render order
    /// (Runtimes phase first, then Tools, stable within each phase).
    pub supplements: Vec<PromptSupplement>,
    /// Declared parameter names available for `${{ parameters.NAME }}`
    /// substitution. Names not in this list are left verbatim by
    /// `prompt.js` with a runtime warning.
    pub parameters: Vec<String>,
}

/// One block of additional prompt content contributed by an extension.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PromptSupplement {
    /// Extension display name (used for VSO logging only — not rendered).
    pub name: String,
    /// Markdown to append. May contain `${{ parameters.* }}` or `$(VAR)`
    /// references; substituted by `prompt.js` using the same rules as
    /// the body.
    pub content: String,
}

/// Generate the JSON Schema for [`PromptSpec`] (consumed by the
/// TS workspace's codegen step).
pub fn generate_prompt_spec_schema() -> String {
    let schema = schemars::schema_for!(PromptSpec);
    serde_json::to_string_pretty(&schema)
        .expect("PromptSpec schema must serialize")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_is_valid_json() {
        let s = generate_prompt_spec_schema();
        let _: serde_json::Value =
            serde_json::from_str(&s).expect("schema must be valid JSON");
    }

    #[test]
    fn version_is_pinned() {
        assert_eq!(PROMPT_SPEC_VERSION, 1);
    }

    #[test]
    fn schema_contains_expected_top_level_fields() {
        let s = generate_prompt_spec_schema();
        // Sanity check that key field names appear in the generated schema
        // (json2ts will rely on these to produce TS types).
        assert!(s.contains("\"version\""));
        assert!(s.contains("\"source_path\""));
        assert!(s.contains("\"output_path\""));
        assert!(s.contains("\"supplements\""));
        assert!(s.contains("\"parameters\""));
    }

    #[test]
    fn spec_serializes_to_expected_json_keys() {
        let spec = PromptSpec {
            version: PROMPT_SPEC_VERSION,
            source_path: "/tmp/x.md".into(),
            output_path: "/tmp/y.md".into(),
            supplements: vec![PromptSupplement {
                name: "Demo".into(),
                content: "demo".into(),
            }],
            parameters: vec!["foo".into()],
        };
        let json = serde_json::to_string(&spec).unwrap();
        assert!(json.contains("\"version\":1"));
        assert!(json.contains("\"source_path\":\"/tmp/x.md\""));
        assert!(json.contains("\"output_path\":\"/tmp/y.md\""));
        assert!(json.contains("\"supplements\""));
        assert!(json.contains("\"parameters\":[\"foo\"]"));
    }
}
