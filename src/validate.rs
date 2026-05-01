//! Centralized input validation primitives.
//!
//! This module is the single source of truth for structural input validators:
//! character allowlists, format validators, injection detectors, and
//! container/DNS validators.  It is shared across the compiler (front matter,
//! engine, MCP), Stage 3 execution, and tests.
//!
//! **Contrast with `sanitize.rs`:** that module transforms content
//! (remove ANSI codes, neutralize mentions, etc.) and returns a cleaned
//! `String`.  This module checks whether input is structurally valid and
//! returns `bool` or `Result`.

use anyhow::Result;
use std::collections::HashMap;

// ── Character allowlist validators ──────────────────────────────────────────

/// Validate that a string is safe to embed as a single path segment (e.g. a
/// repository alias appended to `$(Build.SourcesDirectory)`). Rejects empty
/// strings, anything containing `..`, path separators (`/`, `\`), or leading
/// `.` to prevent path traversal / hidden-directory escapes.
pub fn is_safe_path_segment(s: &str) -> bool {
    !s.is_empty()
        && !s.contains("..")
        && !s.contains('/')
        && !s.contains('\\')
        && !s.starts_with('.')
}

/// Characters allowed in engine.command paths (absolute path chars only).
/// Prevents shell injection when the path is embedded in AWF single-quoted commands.
pub fn is_valid_command_path(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'))
}

/// Characters allowed in engine.agent and engine.model identifiers.
pub fn is_valid_identifier(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-'))
}

/// Characters allowed in engine.api-target hostnames.
pub fn is_valid_hostname(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
}

/// Characters allowed in engine.version strings (e.g., "1.0.34", "latest").
pub fn is_valid_version(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Characters allowed in individual engine.args entries.
/// Strict allowlist to prevent shell injection inside AWF single-quoted commands.
pub fn is_valid_arg(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-' | '=' | '/' | '@'))
}

// ── Format validators ───────────────────────────────────────────────────────

/// Validate that a string is a valid ADO pipeline parameter name (`[A-Za-z_][A-Za-z0-9_]*`).
pub fn is_valid_parameter_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Validate that a string is a legal environment variable name (`[A-Za-z_][A-Za-z0-9_]*`).
/// Prevents injection of arbitrary Docker flags via user-controlled front matter keys.
pub fn is_valid_env_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Returns true if the name contains only ASCII alphanumerics and hyphens.
/// This value is embedded inline in a shell command, so control characters
/// (including newlines) and whitespace must be rejected to prevent corruption.
pub fn is_safe_tool_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

// ── Injection detectors ─────────────────────────────────────────────────────

/// Returns true if the string contains an ADO template expression (`${{`),
/// macro expression (`$(`), or runtime expression (`$[`).
pub fn contains_ado_expression(s: &str) -> bool {
    s.contains("${{") || s.contains("$(") || s.contains("$[")
}

/// Returns true if the string contains an ADO pipeline command
/// (`##vso[` or `##[`).
pub fn contains_pipeline_command(s: &str) -> bool {
    s.contains("##vso[") || s.contains("##[")
}

/// Returns true if the string contains a newline (`\n`) or carriage return (`\r`).
pub fn contains_newline(s: &str) -> bool {
    s.contains('\n') || s.contains('\r')
}

/// Returns true if the string contains the compiler's template marker
/// delimiter (`{{`).  Values substituted into the pipeline template must
/// not contain this sequence — otherwise a second-order substitution can
/// inject arbitrary content (e.g., `{{ agent_content }}` in the `name`
/// field would be expanded by a later replacement pass).
pub fn contains_template_marker(s: &str) -> bool {
    s.contains("{{")
}

/// Reject ADO template expressions (`${{`), macro expressions (`$(`), and runtime
/// expressions (`$[`) in a string value. Parameter definitions should only contain
/// literal values — expressions could enable information disclosure or logic manipulation
/// in the generated pipeline.
pub fn reject_ado_expressions(value: &str, param_name: &str, field_name: &str) -> Result<()> {
    if contains_ado_expression(value) {
        anyhow::bail!(
            "Parameter '{}' field '{}' contains an ADO expression ('${{{{', '$(', or '$[') which \
             is not allowed in parameter definitions. Use literal values only.",
            param_name,
            field_name,
        );
    }
    Ok(())
}

/// Reject ADO expressions in a serde_yaml::Value, recursing into strings within sequences.
pub fn reject_ado_expressions_in_value(
    value: &serde_yaml::Value,
    param_name: &str,
    field_name: &str,
) -> Result<()> {
    match value {
        serde_yaml::Value::String(s) => reject_ado_expressions(s, param_name, field_name),
        serde_yaml::Value::Sequence(seq) => {
            for item in seq {
                reject_ado_expressions_in_value(item, param_name, field_name)?;
            }
            Ok(())
        }
        // Booleans, numbers, null — safe, no injection risk
        _ => Ok(()),
    }
}

/// Reject values that could cause pipeline injection: ADO expressions,
/// pipeline commands (`##vso[`, `##[`), template markers (`{{`), and
/// newlines.  A combined check for fields embedded into YAML templates.
pub fn reject_pipeline_injection(value: &str, field_name: &str) -> Result<()> {
    if contains_ado_expression(value) {
        anyhow::bail!(
            "Front matter '{}' contains an ADO expression ('${{{{', '$(', or '$[') which is not allowed. \
             Use literal values only. Found: '{}'",
            field_name,
            value,
        );
    }
    if contains_pipeline_command(value) {
        anyhow::bail!(
            "Front matter '{}' contains an ADO pipeline command ('##vso[' or '##[') which is not allowed. \
             Pipeline commands could manipulate pipeline behavior. Found: '{}'",
            field_name,
            value,
        );
    }
    if contains_template_marker(value) {
        anyhow::bail!(
            "Front matter '{}' contains a template marker delimiter '{{{{' which is not allowed. \
             Template markers could cause second-order injection into the generated pipeline. Found: '{}'",
            field_name,
            value,
        );
    }
    if contains_newline(value) {
        anyhow::bail!(
            "Front matter '{}' must be a single line (no newlines). \
             Multi-line values could inject YAML structure into the generated pipeline.",
            field_name,
        );
    }
    Ok(())
}

// ── DNS / domain validators ─────────────────────────────────────────────────

/// Validate that a string is a valid DNS-safe domain name or wildcard pattern
/// (`*.example.com`).  Only ASCII alphanumerics, `.`, `-`, and `*` are allowed.
/// Wildcards must appear only as a leading `*.` prefix.
pub fn validate_dns_domain(host: &str) -> Result<()> {
    let valid_chars = !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '*'));
    if !valid_chars {
        anyhow::bail!(
            "network.allowed domain '{}' contains characters invalid in DNS names. \
             Only ASCII alphanumerics, '.', '-', and '*' are allowed.",
            host
        );
    }
    if host.contains('*') && (!host.starts_with("*.") || host[2..].contains('*')) {
        anyhow::bail!(
            "network.allowed domain '{}' uses '*' in an unsupported position. \
             Wildcards must appear only as a leading prefix (e.g. '*.example.com').",
            host
        );
    }
    Ok(())
}

// ── Container / Docker validators ───────────────────────────────────────────

/// Sensitive host path prefixes that should not be bind-mounted into MCP containers.
pub const SENSITIVE_MOUNT_PREFIXES: &[&str] = &[
    "/etc",
    "/root",
    "/home",
    "/proc",
    "/sys",
];

/// Docker runtime flag names that grant dangerous host access.
/// Checked both as `--flag=value` and as `--flag value` (split across two args).
pub const DANGEROUS_DOCKER_FLAGS: &[&str] = &[
    "--privileged",
    "--cap-add",
    "--security-opt",
    "--pid",
    "--network",
    "--ipc",
    "--user",
    "-u",
    "--add-host",
    "--entrypoint",
];

/// Validate a container image name for injection attempts.
/// Allows `[a-zA-Z0-9./_:-@]` which covers standard Docker image references.
pub fn validate_container_image(image: &str, mcp_name: &str) -> Vec<String> {
    let mut warnings = Vec::new();
    if image.is_empty() {
        warnings.push(format!("Warning: MCP '{}': container image name is empty.", mcp_name));
        return warnings;
    }
    if !image.chars().all(|c| c.is_ascii_alphanumeric() || "._/:-@".contains(c)) {
        warnings.push(format!(
            "Warning: MCP '{}': container image '{}' contains unexpected characters. \
            Image names should only contain [a-zA-Z0-9./_:-@].",
            mcp_name, image
        ));
    }
    warnings
}

/// Validate a volume mount source path, warning on sensitive host directories.
/// Docker socket mounts are escalated to stderr warnings since they grant container escape.
/// Note: paths are lowercased for comparison to catch cross-platform casing (e.g. `/ETC/shadow`).
pub fn validate_mount_source(mount: &str, mcp_name: &str) -> Vec<String> {
    let mut warnings = Vec::new();
    // Format: "source:dest:mode"
    if let Some(source) = mount.split(':').next() {
        let source_lower = source.to_lowercase();
        if source_lower.contains("docker.sock") {
            warnings.push(format!(
                "Warning: MCP '{}': mount '{}' exposes the Docker socket to the MCP container. \
                This grants full host Docker access and may allow container escape.",
                mcp_name, mount
            ));
            return warnings;
        }
        for prefix in SENSITIVE_MOUNT_PREFIXES {
            // Match exact path or path with trailing separator to avoid false positives
            // (e.g. /etc matches /etc and /etc/shadow, but not /etc-configs)
            if source_lower == *prefix || source_lower.starts_with(&format!("{}/", prefix)) {
                warnings.push(format!(
                    "Warning: MCP '{}': mount source '{}' references a sensitive host path ({}). \
                    Ensure this is intentional.",
                    mcp_name, source, prefix
                ));
                break;
            }
        }
    }
    warnings
}

/// Validate Docker runtime args for dangerous flags that could escalate privileges.
/// Also detects volume mounts smuggled via `-v`/`--volume` that bypass `mounts` validation.
/// Handles both `--flag=value` and `--flag value` (split) forms.
pub fn validate_docker_args(args: &[String], mcp_name: &str) -> Vec<String> {
    let mut warnings = Vec::new();
    for (i, arg) in args.iter().enumerate() {
        let arg_lower = arg.to_lowercase();
        // Check for dangerous Docker flags (both --flag=value and --flag value)
        for dangerous in DANGEROUS_DOCKER_FLAGS {
            if arg_lower == *dangerous
                || arg_lower.starts_with(&format!("{}=", dangerous))
            {
                let extra_hint = if *dangerous == "--entrypoint" {
                    " Use the 'entrypoint:' field instead of passing --entrypoint in args."
                } else {
                    ""
                };
                warnings.push(format!(
                    "Warning: MCP '{}': Docker arg '{}' grants elevated privileges. \
                    Ensure this is intentional.{}",
                    mcp_name, arg, extra_hint
                ));
            }
        }
        // Check for volume mounts smuggled via args (bypasses mounts validation)
        if arg == "-v" || arg == "--volume" {
            if let Some(mount_spec) = args.get(i + 1) {
                warnings.push(format!(
                    "Warning: MCP '{}': volume mount '{}' in args bypasses mounts validation. \
                    Use the 'mounts:' field instead.",
                    mcp_name, mount_spec
                ));
                warnings.extend(validate_mount_source(mount_spec, mcp_name));
            } else {
                warnings.push(format!(
                    "Warning: MCP '{}': '{}' flag is the last arg with no mount spec following it. \
                    This is likely a malformed args list.",
                    mcp_name, arg
                ));
            }
        } else if arg_lower.starts_with("-v=") || arg_lower.starts_with("--volume=") {
            let mount_spec = arg.split_once('=').map_or("", |(_, v)| v);
            warnings.push(format!(
                "Warning: MCP '{}': volume mount '{}' in args bypasses mounts validation. \
                Use the 'mounts:' field instead.",
                mcp_name, mount_spec
            ));
            warnings.extend(validate_mount_source(mount_spec, mcp_name));
        }
    }
    warnings
}

/// Validate that an MCP HTTP URL uses an allowed scheme.
pub fn validate_mcp_url(url: &str, mcp_name: &str) -> Vec<String> {
    let mut warnings = Vec::new();
    if !url.starts_with("https://") && !url.starts_with("http://") {
        warnings.push(format!(
            "Warning: MCP '{}': URL '{}' does not use http:// or https:// scheme. \
            This may not work with MCPG.",
            mcp_name, url
        ));
    }
    warnings
}

/// Warn when env values or headers look like they contain inline secrets.
/// Secrets should use pipeline variables and passthrough ("") instead.
pub fn warn_potential_secrets(mcp_name: &str, env: &HashMap<String, String>, headers: &HashMap<String, String>) -> Vec<String> {
    let mut warnings = Vec::new();
    for (key, value) in env {
        if !value.is_empty() && (key.to_lowercase().contains("token")
            || key.to_lowercase().contains("secret")
            || key.to_lowercase().contains("key")
            || key.to_lowercase().contains("password")
            || key.to_lowercase().contains("pat"))
        {
            warnings.push(format!(
                "Warning: MCP '{}': env var '{}' has an inline value that may be a secret. \
                Use an empty string (\"\") for passthrough from pipeline variables instead.",
                mcp_name, key
            ));
        }
    }
    for (key, value) in headers {
        if value.to_lowercase().contains("bearer ")
            || key.to_lowercase() == "authorization"
        {
            warnings.push(format!(
                "Warning: MCP '{}': header '{}' may contain inline credentials. \
                These will appear in plaintext in the compiled pipeline YAML.",
                mcp_name, key
            ));
        }
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Character allowlist validators ──────────────────────────────────

    #[test]
    fn test_is_safe_path_segment() {
        assert!(is_safe_path_segment("my-repo"));
        assert!(is_safe_path_segment("exp23-a7-nw"));
        assert!(is_safe_path_segment("repo_v2"));
        assert!(!is_safe_path_segment(""));
        assert!(!is_safe_path_segment(".."));
        assert!(!is_safe_path_segment("../sibling"));
        assert!(!is_safe_path_segment("foo/bar"));
        assert!(!is_safe_path_segment("foo\\bar"));
        assert!(!is_safe_path_segment(".hidden"));
        assert!(!is_safe_path_segment("foo..bar"));
    }

    #[test]
    fn test_is_valid_command_path() {
        assert!(is_valid_command_path("/tmp/awf-tools/copilot"));
        assert!(is_valid_command_path("copilot"));
        assert!(is_valid_command_path("/usr/local/bin/my-tool_v2"));
        assert!(!is_valid_command_path(""));
        assert!(!is_valid_command_path("/tmp/copilot; rm -rf /"));
        assert!(!is_valid_command_path("/tmp/copilot'"));
        assert!(!is_valid_command_path("path with spaces"));
    }

    #[test]
    fn test_is_valid_identifier() {
        assert!(is_valid_identifier("claude-opus-4.5"));
        assert!(is_valid_identifier("gpt-5.2-codex"));
        assert!(is_valid_identifier("my-agent"));
        assert!(is_valid_identifier("model:variant"));
        assert!(!is_valid_identifier(""));
        assert!(!is_valid_identifier("bad agent!"));
        assert!(!is_valid_identifier("model with spaces"));
    }

    #[test]
    fn test_is_valid_hostname() {
        assert!(is_valid_hostname("api.github.com"));
        assert!(is_valid_hostname("api.acme-ghe.com"));
        assert!(is_valid_hostname("localhost"));
        assert!(!is_valid_hostname(""));
        assert!(!is_valid_hostname("host/path"));
        assert!(!is_valid_hostname("host:443"));
    }

    #[test]
    fn test_is_valid_version() {
        assert!(is_valid_version("1.0.34"));
        assert!(is_valid_version("latest"));
        assert!(is_valid_version("1.0.0-beta"));
        assert!(is_valid_version("1.0.0_rc1"));
        assert!(!is_valid_version(""));
        assert!(!is_valid_version("1.0.0 -Source https://evil.com"));
    }

    #[test]
    fn test_is_valid_arg() {
        assert!(is_valid_arg("--verbose"));
        assert!(is_valid_arg("--option=value"));
        assert!(is_valid_arg("--path=/dir/file"));
        assert!(is_valid_arg("--email=user@domain"));
        assert!(!is_valid_arg(""));
        assert!(!is_valid_arg("--flag; rm -rf /"));
        assert!(!is_valid_arg("arg with spaces"));
    }

    // ── Format validators ───────────────────────────────────────────────

    #[test]
    fn test_is_valid_parameter_name() {
        assert!(is_valid_parameter_name("clearMemory"));
        assert!(is_valid_parameter_name("_private"));
        assert!(is_valid_parameter_name("param123"));
        assert!(!is_valid_parameter_name(""));
        assert!(!is_valid_parameter_name("123abc"));
        assert!(!is_valid_parameter_name("my-param"));
        assert!(!is_valid_parameter_name("my param"));
    }

    #[test]
    fn test_is_valid_env_var_name() {
        assert!(is_valid_env_var_name("MY_VAR"));
        assert!(is_valid_env_var_name("_PRIVATE"));
        assert!(is_valid_env_var_name("A"));
        assert!(is_valid_env_var_name("VAR123"));
        assert!(!is_valid_env_var_name(""));
        assert!(!is_valid_env_var_name("123ABC"));
        assert!(!is_valid_env_var_name("MY-VAR"));
        assert!(!is_valid_env_var_name("MY VAR"));
        assert!(!is_valid_env_var_name("X --privileged"));
        assert!(!is_valid_env_var_name("X -v /etc:/etc:rw"));
    }

    #[test]
    fn test_is_safe_tool_name() {
        assert!(is_safe_tool_name("create-pull-request"));
        assert!(is_safe_tool_name("noop"));
        assert!(is_safe_tool_name("my-tool-123"));
        assert!(!is_safe_tool_name(""));
        assert!(!is_safe_tool_name("$(curl evil.com)"));
        assert!(!is_safe_tool_name("foo; rm -rf /"));
        assert!(!is_safe_tool_name("tool name"));
        assert!(!is_safe_tool_name("tool\ttab"));
    }

    // ── Injection detectors ─────────────────────────────────────────────

    #[test]
    fn test_contains_ado_expression() {
        assert!(contains_ado_expression("${{ variables.x }}"));
        assert!(contains_ado_expression("$(SYSTEM_ACCESSTOKEN)"));
        assert!(contains_ado_expression("$[variables.x]"));
        assert!(!contains_ado_expression("normal text"));
        assert!(!contains_ado_expression("$100"));
    }

    #[test]
    fn test_contains_pipeline_command() {
        assert!(contains_pipeline_command("##vso[task.setvariable]x"));
        assert!(contains_pipeline_command("##[section]foo"));
        assert!(!contains_pipeline_command("normal text"));
        assert!(!contains_pipeline_command("## heading"));
    }

    #[test]
    fn test_contains_newline() {
        assert!(contains_newline("line1\nline2"));
        assert!(contains_newline("line1\rline2"));
        assert!(!contains_newline("single line"));
    }

    #[test]
    fn test_contains_template_marker() {
        assert!(contains_template_marker("{{ agent_content }}"));
        assert!(contains_template_marker("prefix {{ something }} suffix"));
        assert!(contains_template_marker("{{no_spaces}}"));
        assert!(!contains_template_marker("normal text"));
        assert!(!contains_template_marker("just a single { brace"));
    }

    #[test]
    fn test_reject_ado_expressions() {
        assert!(reject_ado_expressions("normal value", "param", "field").is_ok());
        assert!(reject_ado_expressions("$(SYSTEM_ACCESSTOKEN)", "param", "field").is_err());
        assert!(reject_ado_expressions("${{ variables.x }}", "param", "field").is_err());
        assert!(reject_ado_expressions("$[variables.x]", "param", "field").is_err());
    }

    #[test]
    fn test_reject_pipeline_injection() {
        assert!(reject_pipeline_injection("normal value", "field").is_ok());
        assert!(reject_pipeline_injection("$(SYSTEM_ACCESSTOKEN)", "field").is_err());
        assert!(reject_pipeline_injection("value\ninjected", "field").is_err());
        assert!(reject_pipeline_injection("{{ agent_content }}", "field").is_err());
        assert!(reject_pipeline_injection("{{ copilot_params }}", "field").is_err());
        assert!(reject_pipeline_injection("##vso[task.setvariable]x", "field").is_err());
        assert!(reject_pipeline_injection("##[section]foo", "field").is_err());
    }

    // ── DNS domain validators ───────────────────────────────────────────

    #[test]
    fn test_validate_dns_domain() {
        assert!(validate_dns_domain("github.com").is_ok());
        assert!(validate_dns_domain("*.example.com").is_ok());
        assert!(validate_dns_domain("api.acme-corp.com").is_ok());
        assert!(validate_dns_domain("").is_err());
        assert!(validate_dns_domain("host/path").is_err());
        assert!(validate_dns_domain("host with spaces").is_err());
        assert!(validate_dns_domain("*evil*").is_err());
        assert!(validate_dns_domain("foo.*.com").is_err());
    }

    // ── Container / Docker validators ───────────────────────────────────

    #[test]
    fn test_validate_container_image() {
        assert!(validate_container_image("node:20-slim", "mcp").is_empty());
        assert!(validate_container_image("ghcr.io/org/tool:latest", "mcp").is_empty());
        assert!(!validate_container_image("", "mcp").is_empty());
        assert!(!validate_container_image("$(malicious)", "mcp").is_empty());
    }

    #[test]
    fn test_validate_docker_args_privileged_flag() {
        let warnings = validate_docker_args(&["--privileged".to_string()], "my-mcp");
        assert!(!warnings.is_empty());
        assert!(warnings[0].contains("elevated privileges"));
    }

    #[test]
    fn test_validate_docker_args_entrypoint_in_args_warns() {
        let warnings = validate_docker_args(
            &["--entrypoint".to_string(), "/bin/sh".to_string()],
            "my-mcp",
        );
        assert!(!warnings.is_empty());
        assert!(warnings[0].contains("entrypoint"));
    }

    #[test]
    fn test_validate_docker_args_volume_flag_calls_mount_validation() {
        let warnings = validate_docker_args(
            &["-v".to_string(), "/etc/passwd:/data:ro".to_string()],
            "my-mcp",
        );
        assert!(warnings.len() >= 2); // bypass warning + sensitive path
        assert!(warnings[0].contains("bypasses mounts"));
    }

    #[test]
    fn test_validate_mcp_url() {
        assert!(validate_mcp_url("https://mcp.example.com", "mcp").is_empty());
        assert!(validate_mcp_url("http://localhost:8080", "mcp").is_empty());
        assert!(!validate_mcp_url("ftp://example.com", "mcp").is_empty());
    }

    #[test]
    fn test_warn_potential_secrets() {
        let env = HashMap::from([("AZURE_DEVOPS_EXT_PAT".to_string(), "secret123".to_string())]);
        let warnings = warn_potential_secrets("mcp", &env, &HashMap::new());
        assert!(!warnings.is_empty());
        assert!(warnings[0].contains("secret"));

        let empty_env = HashMap::from([("AZURE_DEVOPS_EXT_PAT".to_string(), String::new())]);
        let warnings = warn_potential_secrets("mcp", &empty_env, &HashMap::new());
        assert!(warnings.is_empty());
    }
}
