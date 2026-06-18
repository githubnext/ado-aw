//! Input sanitization layer (spec section 4)
//!
//! Implements the sanitization pipeline defined in `sanitize-spec.md` to protect
//! against template injection and prompt injection in Azure DevOps contexts.
//! This module is shared across Stage 1 (safe output creation), threat analysis
//! ingestion, and Stage 3 (safe output execution).
//!
//! Two traits cover different trust boundaries:
//!
//! - [`SanitizeContent`] — for agent-generated content (safe-output results).
//!   Applies the full sanitization pipeline including HTML escaping, @mention
//!   wrapping, bot trigger neutralization, etc.
//! - [`SanitizeConfig`] — for operator-controlled configuration values (front
//!   matter and safe-output configs). Applies a lighter pipeline that protects
//!   against pipeline command injection and control characters without corrupting
//!   identifiers like area paths, wiki names, or assignee emails.

use log::debug;
use std::ops::Range;

/// Trait for types that contain untrusted agent-generated text fields.
///
/// Implement this on safe output result structs so Stage 3 execution can
/// call `sanitize_content_fields()` before dispatching to Azure DevOps APIs.
///
/// Use `#[derive(SanitizeContent)]` from the `ado-aw-derive` crate for automatic
/// implementation on structs with named fields.
pub trait SanitizeContent {
    /// Apply the full sanitization pipeline to all untrusted content fields in-place.
    fn sanitize_content_fields(&mut self);
}

/// Trait for types that contain operator-controlled configuration text fields.
///
/// Implement this on front matter structs and safe-output config structs so
/// that all textual values are sanitized before use in template substitution
/// or API calls.
///
/// Use `#[derive(SanitizeConfig)]` from the `ado-aw-derive` crate for automatic
/// implementation on structs with named fields.
pub trait SanitizeConfig {
    /// Apply the config-appropriate sanitization pipeline to all text fields in-place.
    fn sanitize_config_fields(&mut self);
}

/// Maximum content size in bytes (IS-08)
const MAX_CONTENT_BYTES: usize = 524_288; // 0.5 MB

/// Maximum line count (IS-08)
const MAX_LINE_COUNT: usize = 65_536;

/// Run the full sanitization pipeline on untrusted input (IS-10).
///
/// Steps executed in order:
/// 1. Remove ANSI escape sequences and control characters (IS-09)
/// 2. Neutralize @mentions (IS-04)
/// 3. Neutralize bot triggers and work item links (IS-05)
/// 4. Remove XML comments (IS-06b)
/// 5. Convert HTML/XML tags to entities (IS-06)
/// 6. Sanitize URL protocols (IS-07b)
/// 7. Apply content size limits (IS-08)
pub fn sanitize(input: &str) -> String {
    let mut s = remove_control_characters(input);
    s = neutralize_pipeline_commands(&s);
    s = neutralize_mentions(&s);
    s = neutralize_bot_triggers(&s);
    s = remove_xml_comments(&s);
    s = escape_html_tags(&s);
    s = sanitize_url_protocols(&s);
    s = enforce_content_limits(&s);
    debug!("Sanitized content: {} -> {} bytes", input.len(), s.len());
    s
}

/// Sanitize operator-controlled configuration values.
///
/// Applies a subset of the full pipeline appropriate for config identifiers:
/// 1. Remove ANSI escape sequences and control characters (IS-09)
/// 2. Neutralize pipeline commands (`##vso[`, `##[`)
/// 3. Apply content size limits (IS-08)
///
/// Skips HTML escaping, @mention wrapping, bot trigger neutralization, XML
/// comment removal, and URL protocol sanitization — these are content-rendering
/// concerns that would corrupt identifiers like area paths, wiki names, or emails.
pub fn sanitize_config(input: &str) -> String {
    let mut s = remove_control_characters(input);
    s = neutralize_pipeline_commands(&s);
    s = enforce_content_limits(&s);
    debug!(
        "Sanitized config value: {} -> {} bytes",
        input.len(),
        s.len()
    );
    s
}

// ── IS-09: Control character & ANSI escape removal ─────────────────────────

/// Remove ANSI escape sequences and unsafe control characters.
/// Preserves newline (0x0A), tab (0x09), and carriage return (0x0D).
fn remove_control_characters(input: &str) -> String {
    // First strip ANSI escape sequences (e.g. \x1b[31m)
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Skip the escape sequence: consume until a letter terminates it
            // Handles CSI sequences: ESC [ ... <letter>
            if chars.peek() == Some(&'[') {
                chars.next(); // consume '['
                while let Some(&next) = chars.peek() {
                    chars.next();
                    if next.is_ascii_alphabetic() || next == '~' {
                        break;
                    }
                }
            }
            // else: standalone ESC, just drop it
            continue;
        }

        // Strip ASCII control characters except tab, newline, carriage return
        if c.is_ascii_control() && c != '\n' && c != '\t' && c != '\r' {
            continue;
        }

        result.push(c);
    }

    result
}

// ── Azure DevOps pipeline command neutralization ───────────────────────────

/// Neutralize `##vso[` logging command sequences that Azure DevOps interprets
/// when they appear in pipeline stdout/stderr. Wraps them in backticks so they
/// render as code instead of being executed.
///
/// Also handles `##[` (the shorthand form used for `##[section]`, `##[error]`,
/// etc.) which ADO pipelines also interpret.
pub(crate) fn neutralize_pipeline_commands(input: &str) -> String {
    let mut result = String::with_capacity(input.len() + 32);
    let mut rest = input;

    while let Some(pos) = rest.find("##") {
        result.push_str(&rest[..pos]);
        let after = &rest[pos + 2..];
        if let Some(stripped) = after.strip_prefix("vso[") {
            result.push_str("`##vso[`");
            rest = stripped;
        } else if let Some(stripped) = after.strip_prefix('[') {
            result.push_str("`##[`");
            rest = stripped;
        } else {
            // Harmless "##" (e.g. markdown heading)
            result.push_str("##");
            rest = after;
        }
    }
    result.push_str(rest);
    result
}

// ── IS-04: @mention neutralization ─────────────────────────────────────────

/// Wrap @mentions in backticks to prevent unintended notifications.
fn neutralize_mentions(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.char_indices().peekable();

    while let Some((i, c)) = chars.next() {
        if c == '@' {
            // Don't neutralize if already inside backticks
            let before = &input[..i];
            let open_backticks = before.matches('`').count();
            if open_backticks % 2 == 1 {
                // Inside inline code – leave as is
                result.push(c);
                continue;
            }

            // Collect the mention word
            let mut mention = String::from("@");
            while let Some(&(_, next_c)) = chars.peek() {
                if next_c.is_alphanumeric() || next_c == '_' || next_c == '-' || next_c == '.' {
                    mention.push(next_c);
                    chars.next();
                } else {
                    break;
                }
            }

            if mention.len() > 1 {
                // Wrap in backticks
                result.push('`');
                result.push_str(&mention);
                result.push('`');
            } else {
                // Bare '@' with no username – keep as-is
                result.push('@');
            }
        } else {
            result.push(c);
        }
    }

    result
}

// ── IS-05: Bot trigger and work item link protection ───────────────────────

use std::sync::LazyLock;

static RE_BOT_KEYWORDS: LazyLock<regex_lite::Regex> = LazyLock::new(|| {
    regex_lite::Regex::new(r"(?i)\b(fix(?:es)?|close[sd]?|resolve[sd]?)\s+(#\d+)").unwrap()
});
static RE_AB_LINK: LazyLock<regex_lite::Regex> =
    LazyLock::new(|| regex_lite::Regex::new(r"\bAB#(\d+)").unwrap());
static RE_SLASH_CMD: LazyLock<regex_lite::Regex> =
    LazyLock::new(|| regex_lite::Regex::new(r"(?m)^(/[a-zA-Z][\w-]*)").unwrap());

/// Neutralize bot command patterns and Azure DevOps work item link syntax.
fn neutralize_bot_triggers(input: &str) -> String {
    let s = RE_BOT_KEYWORDS
        .replace_all(input, |caps: &regex_lite::Captures| {
            format!("`{} {}`", &caps[1], &caps[2])
        })
        .to_string();

    let s = RE_AB_LINK
        .replace_all(&s, |caps: &regex_lite::Captures| {
            format!("`AB#{}`", &caps[1])
        })
        .to_string();

    RE_SLASH_CMD
        .replace_all(&s, |caps: &regex_lite::Captures| format!("`{}`", &caps[1]))
        .to_string()
}

// ── IS-06: HTML/XML tag filtering ──────────────────────────────────────────

/// Remove XML/HTML comments (IS-06b). Must run before tag conversion.
fn remove_xml_comments(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut rest = input;

    while let Some(start) = rest.find("<!--") {
        result.push_str(&rest[..start]);
        if let Some(end) = rest[start..].find("-->") {
            rest = &rest[start + end + 3..];
        } else {
            // Unclosed comment – remove to end
            rest = "";
        }
    }
    result.push_str(rest);
    result
}

/// Convert HTML/XML tags to safe HTML entities (IS-06).
fn escape_html_tags(input: &str) -> String {
    let protected = markdown_protected_ranges(input);
    if protected.is_empty() {
        return escape_html_fragment(input);
    }

    let mut result = String::with_capacity(input.len());
    let mut cursor = 0;
    for range in protected {
        result.push_str(&escape_html_fragment(&input[cursor..range.start]));
        result.push_str(&input[range.start..range.end]);
        cursor = range.end;
    }
    result.push_str(&escape_html_fragment(&input[cursor..]));
    result
}

fn escape_html_fragment(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut rest = input;

    while let Some(start) = rest.find('<') {
        result.push_str(&rest[..start]);
        if let Some(end) = rest[start..].find('>') {
            let tag = &rest[start..start + end + 1];
            result.push_str("&lt;");
            result.push_str(&tag[1..tag.len() - 1]);
            result.push_str("&gt;");
            rest = &rest[start + end + 1..];
        } else {
            // No closing '>' – escape the lone '<'
            result.push_str("&lt;");
            rest = &rest[start + 1..];
        }
    }
    result.push_str(rest);
    result
}

fn markdown_protected_ranges(input: &str) -> Vec<Range<usize>> {
    let fence_ranges = fenced_code_ranges(input);
    let mut ranges = Vec::new();
    let mut cursor = 0;

    for fence in &fence_ranges {
        if cursor < fence.start {
            collect_inline_code_ranges(input, cursor, fence.start, &mut ranges);
        }
        ranges.push(fence.clone());
        cursor = fence.end;
    }

    if cursor < input.len() {
        collect_inline_code_ranges(input, cursor, input.len(), &mut ranges);
    }

    ranges
}

fn fenced_code_ranges(input: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut line_start = 0;

    while line_start < input.len() {
        let (line_end, next_line_start) = line_bounds(input, line_start);
        let line = &input[line_start..line_end];

        if let Some((marker, count)) = parse_fence_opener(line)
            && let Some(block_end) = find_matching_fence_end(input, next_line_start, marker, count)
        {
            ranges.push(line_start..block_end);
            line_start = block_end;
            continue;
        }

        line_start = next_line_start;
    }

    ranges
}

fn collect_inline_code_ranges(
    input: &str,
    start: usize,
    end: usize,
    ranges: &mut Vec<Range<usize>>,
) {
    let bytes = input.as_bytes();
    let mut i = start;

    while i < end {
        if bytes[i] != b'`' {
            i += 1;
            continue;
        }

        let tick_count = count_repeated_byte(bytes, i, end, b'`');
        let inline_code_boundary = input[i..end]
            .find('\n')
            .map(|offset| i + offset)
            .unwrap_or(end);
        let mut cursor = i + tick_count;
        let mut matched_end = None;

        while cursor < inline_code_boundary {
            if bytes[cursor] == b'`' {
                let candidate_count =
                    count_repeated_byte(bytes, cursor, inline_code_boundary, b'`');
                if candidate_count == tick_count {
                    matched_end = Some(cursor + candidate_count);
                    break;
                }
                cursor += candidate_count;
            } else {
                cursor += 1;
            }
        }

        if let Some(span_end) = matched_end {
            ranges.push(i..span_end);
            i = span_end;
        } else {
            i += tick_count;
        }
    }
}

fn line_bounds(input: &str, line_start: usize) -> (usize, usize) {
    let line_end = input[line_start..]
        .find('\n')
        .map(|offset| line_start + offset)
        .unwrap_or(input.len());
    let next_line_start = if line_end < input.len() {
        line_end + 1
    } else {
        input.len()
    };
    (line_end, next_line_start)
}

fn parse_fence_opener(line: &str) -> Option<(u8, usize)> {
    let indent = line.bytes().take_while(|b| *b == b' ').count();
    if indent > 3 {
        return None;
    }

    let rest = &line.as_bytes()[indent..];
    let marker = *rest.first()?;
    if marker != b'`' && marker != b'~' {
        return None;
    }

    let count = rest.iter().take_while(|&&b| b == marker).count();
    (count >= 3).then_some((marker, count))
}

fn find_matching_fence_end(
    input: &str,
    mut line_start: usize,
    marker: u8,
    min_count: usize,
) -> Option<usize> {
    while line_start < input.len() {
        let (line_end, next_line_start) = line_bounds(input, line_start);
        let line = &input[line_start..line_end];
        let indent = line.bytes().take_while(|b| *b == b' ').count();

        if indent <= 3 {
            let rest = &line.as_bytes()[indent..];
            let count = rest.iter().take_while(|&&b| b == marker).count();
            if count >= min_count && rest[count..].iter().all(|b| matches!(b, b' ' | b'\t')) {
                return Some(next_line_start);
            }
        }

        line_start = next_line_start;
    }

    None
}

fn count_repeated_byte(bytes: &[u8], start: usize, end: usize, byte: u8) -> usize {
    let mut count = 0;
    while start + count < end && bytes[start + count] == byte {
        count += 1;
    }
    count
}

// ── IS-07b: URL protocol sanitization ──────────────────────────────────────

/// Strip unsafe URL protocols (javascript:, data:, file:, vbscript:).
fn sanitize_url_protocols(input: &str) -> String {
    let mut s = input.to_string();
    for protocol in &["javascript:", "data:", "file:", "vbscript:"] {
        // Case-insensitive replacement
        let lower = s.to_lowercase();
        let mut new = String::with_capacity(s.len());
        let mut search_from = 0;
        while let Some(pos) = lower[search_from..].find(protocol) {
            let abs_pos = search_from + pos;
            new.push_str(&s[search_from..abs_pos]);
            new.push_str("(redacted)");
            search_from = abs_pos + protocol.len();
        }
        new.push_str(&s[search_from..]);
        s = new;
    }
    s
}

// ── IS-08: Content limits ──────────────────────────────────────────────────

/// Enforce maximum content size and line count limits.
fn enforce_content_limits(input: &str) -> String {
    let mut s = input.to_string();

    // Byte limit
    if s.len() > MAX_CONTENT_BYTES {
        // Truncate at a valid UTF-8 boundary
        let mut end = MAX_CONTENT_BYTES;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
        s.push_str("\n[Content truncated: exceeded maximum size limit]");
    }

    // Line count limit
    let line_count = s.lines().count();
    if line_count > MAX_LINE_COUNT {
        let truncated: String = s
            .lines()
            .take(MAX_LINE_COUNT)
            .collect::<Vec<_>>()
            .join("\n");
        return format!(
            "{}\n[Content truncated: exceeded maximum line count ({} lines)]",
            truncated, line_count
        );
    }

    s
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // IS-09: Control character removal
    #[test]
    fn test_remove_ansi_escape_codes() {
        let input = "Hello \x1b[31mred\x1b[0m world";
        assert_eq!(remove_control_characters(input), "Hello red world");
    }

    #[test]
    fn test_remove_control_characters_preserves_whitespace() {
        let input = "line1\nline2\ttab\r\n";
        assert_eq!(remove_control_characters(input), "line1\nline2\ttab\r\n");
    }

    #[test]
    fn test_remove_null_and_bell() {
        let input = "hello\x00world\x07!";
        assert_eq!(remove_control_characters(input), "helloworld!");
    }

    #[test]
    fn test_remove_delete_character() {
        let input = "hello\x7fworld";
        // ASCII DEL (127) is a control character
        assert_eq!(remove_control_characters(input), "helloworld");
    }

    // IS-04: Mention neutralization
    #[test]
    fn test_neutralize_mentions() {
        assert_eq!(neutralize_mentions("Hello @user"), "Hello `@user`");
    }

    #[test]
    fn test_neutralize_mentions_preserves_inside_backticks() {
        assert_eq!(neutralize_mentions("See `@user` here"), "See `@user` here");
    }

    #[test]
    fn test_neutralize_mentions_bare_at() {
        assert_eq!(neutralize_mentions("email @ domain"), "email @ domain");
    }

    #[test]
    fn test_neutralize_mentions_multiple() {
        assert_eq!(
            neutralize_mentions("@alice and @bob"),
            "`@alice` and `@bob`"
        );
    }

    // IS-05: Bot trigger / work item link protection
    #[test]
    fn test_neutralize_fixes() {
        assert_eq!(neutralize_bot_triggers("fixes #123"), "`fixes #123`");
    }

    #[test]
    fn test_neutralize_closes_case_insensitive() {
        assert_eq!(neutralize_bot_triggers("Closes #456"), "`Closes #456`");
    }

    #[test]
    fn test_neutralize_ab_work_item_link() {
        assert_eq!(
            neutralize_bot_triggers("See AB#789 for details"),
            "See `AB#789` for details"
        );
    }

    #[test]
    fn test_neutralize_slash_command() {
        assert_eq!(
            neutralize_bot_triggers("/approve\nsome text"),
            "`/approve`\nsome text"
        );
    }

    // IS-06: HTML/XML tag filtering
    #[test]
    fn test_escape_html_tags() {
        assert_eq!(
            escape_html_tags("<script>alert(1)</script>"),
            "&lt;script&gt;alert(1)&lt;/script&gt;"
        );
    }

    #[test]
    fn test_escape_self_closing_tag() {
        assert_eq!(escape_html_tags("<br/>"), "&lt;br/&gt;");
    }

    #[test]
    fn test_escape_tag_with_attributes() {
        assert_eq!(
            escape_html_tags(r#"<a href="evil">"#),
            r#"&lt;a href="evil"&gt;"#
        );
    }

    #[test]
    fn test_remove_xml_comments() {
        assert_eq!(
            remove_xml_comments("before<!-- comment -->after"),
            "beforeafter"
        );
    }

    #[test]
    fn test_remove_unclosed_xml_comment() {
        assert_eq!(remove_xml_comments("before<!-- no end"), "before");
    }

    // IS-07b: URL protocol sanitization
    #[test]
    fn test_strip_javascript_protocol() {
        assert_eq!(
            sanitize_url_protocols("javascript:alert(1)"),
            "(redacted)alert(1)"
        );
    }

    #[test]
    fn test_strip_data_protocol() {
        assert_eq!(
            sanitize_url_protocols("data:text/html,<h1>hi</h1>"),
            "(redacted)text/html,<h1>hi</h1>"
        );
    }

    #[test]
    fn test_strip_file_protocol() {
        assert_eq!(
            sanitize_url_protocols("file:///etc/passwd"),
            "(redacted)///etc/passwd"
        );
    }

    #[test]
    fn test_preserves_https() {
        let input = "https://dev.azure.com/org/project";
        assert_eq!(sanitize_url_protocols(input), input);
    }

    #[test]
    fn test_case_insensitive_protocol_stripping() {
        assert_eq!(
            sanitize_url_protocols("JAVASCRIPT:alert(1)"),
            "(redacted)alert(1)"
        );
    }

    // IS-08: Content limits
    #[test]
    fn test_enforce_byte_limit() {
        let big = "a".repeat(MAX_CONTENT_BYTES + 100);
        let result = enforce_content_limits(&big);
        assert!(result.len() < big.len()); // truncation must have shortened the content
        assert!(result.contains("[Content truncated"));
    }

    #[test]
    fn test_enforce_line_limit() {
        let lines: String = (0..MAX_LINE_COUNT + 10)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let result = enforce_content_limits(&lines);
        assert!(result.contains("[Content truncated"));
        // Verify lines were actually truncated (not just the notice appended to the full input)
        assert!(result.lines().count() <= MAX_LINE_COUNT + 2);
    }

    #[test]
    fn test_small_content_unchanged() {
        let input = "Hello world";
        assert_eq!(enforce_content_limits(input), input);
    }

    // Full pipeline
    #[test]
    fn test_sanitize_pipeline_ordering() {
        // Combines multiple threats: ANSI, mention, HTML tag, unsafe protocol
        let input = "\x1b[31m@admin\x1b[0m says <script>javascript:alert(1)</script> fixes #42";
        let result = sanitize(input);

        assert!(!result.contains("\x1b"));
        assert!(result.contains("`@admin`"));
        assert!(result.contains("&lt;script&gt;"));
        assert!(result.contains("(redacted)"));
        assert!(result.contains("`fixes #42`"));
    }

    #[test]
    fn test_sanitize_xml_comment_removed_before_tag_escape() {
        let input = "<!-- <script>evil</script> -->safe";
        let result = sanitize(input);
        // Comment should be removed entirely; no escaped tags from within the comment
        assert_eq!(result, "safe");
    }

    #[test]
    fn test_sanitize_ab_work_item_link() {
        let input = "This relates to AB#12345";
        let result = sanitize(input);
        // Input has no HTML, ANSI, mentions, or unsafe protocols, so only AB# is transformed
        assert_eq!(result, "This relates to `AB#12345`");
    }

    #[test]
    fn test_sanitize_preserves_normal_text() {
        let input = "This is a normal description of a work item.";
        assert_eq!(sanitize(input), input);
    }

    // ── Pipeline command neutralization tests ──────────────────────────────

    #[test]
    fn test_neutralize_vso_command() {
        let input = "##vso[task.setvariable variable=secret]hack";
        let result = sanitize(input);
        // The raw ##vso[ should be wrapped in backticks so ADO won't interpret it
        assert!(result.contains("`##vso[`"));
        // Verify it's not present in its original unescaped form (i.e. without backtick prefix)
        assert!(!result.contains("##vso[task."));
    }

    #[test]
    fn test_neutralize_vso_shorthand() {
        let input = "##[error]Something bad happened";
        let result = sanitize(input);
        assert!(result.contains("`##[`"));
    }

    #[test]
    fn test_sanitize_preserves_markdown_headings() {
        // Markdown headings and bare issue references must not be neutralized.
        // A bare "#123" (without a preceding bot keyword like "fixes") must NOT
        // be wrapped in backticks — this assertion would pass even if "#123"
        // were wrapped, so we assert the exact output instead.
        let input = "# Heading\n## Sub-heading\nIssue #123";
        assert_eq!(sanitize(input), input);
    }

    #[test]
    fn test_escape_html_tags_preserves_inline_code_spans() {
        let input = "Use `<foo>` and <b>bold</b>.";
        assert_eq!(
            escape_html_tags(input),
            "Use `<foo>` and &lt;b&gt;bold&lt;/b&gt;."
        );
    }

    #[test]
    fn test_escape_html_tags_preserves_fenced_code_blocks() {
        let input = "```suggestion\nif (a < b) {\n    return;\n}\n```\n<div>tail</div>";
        assert_eq!(
            escape_html_tags(input),
            "```suggestion\nif (a < b) {\n    return;\n}\n```\n&lt;div&gt;tail&lt;/div&gt;"
        );
    }

    #[test]
    fn test_escape_html_tags_unmatched_inline_backtick_does_not_disable_escaping() {
        let input = "Unclosed `code <b>still escaped</b>";
        assert_eq!(
            escape_html_tags(input),
            "Unclosed `code &lt;b&gt;still escaped&lt;/b&gt;"
        );
    }

    // ── sanitize_config tests ─────────────────────────────────────────────

    #[test]
    fn test_sanitize_config_neutralizes_pipeline_commands() {
        let input = "##vso[task.setvariable variable=secret]value";
        let result = sanitize_config(input);
        assert!(result.contains("`##vso[`"));
        assert!(!result.contains("##vso[task."));
    }

    #[test]
    fn test_sanitize_config_neutralizes_shorthand_pipeline_command() {
        let input = "##[error]bad";
        let result = sanitize_config(input);
        assert!(
            result.contains("`##[`"),
            "##[ shorthand should be wrapped in backticks; got: {}",
            result
        );
        assert!(
            !result.contains("##[error]"),
            "##[error] should be neutralized; got: {}",
            result
        );
    }

    #[test]
    fn test_sanitize_config_removes_control_chars() {
        // ANSI escape sequences must be stripped through the config pipeline.
        // "\x1b[0m" is the ANSI reset code; it should be removed, joining "val" and "ue".
        let input = "val\x1b[0mue";
        assert_eq!(sanitize_config(input), "value");
    }

    #[test]
    fn test_sanitize_config_preserves_html_tags() {
        let input = "area-path: <MyProject>\\Team";
        let result = sanitize_config(input);
        assert!(
            result.contains("<MyProject>"),
            "Config sanitize should NOT escape HTML tags"
        );
    }

    #[test]
    fn test_sanitize_config_preserves_at_mentions() {
        let input = "user@example.com";
        assert_eq!(
            sanitize_config(input),
            input,
            "Config sanitize should NOT wrap @mentions"
        );
    }

    #[test]
    fn test_sanitize_config_preserves_bot_triggers() {
        let input = "fixes #123";
        assert_eq!(
            sanitize_config(input),
            input,
            "Config sanitize should NOT neutralize bot triggers"
        );
    }

    #[test]
    fn test_sanitize_config_preserves_normal_text() {
        let input = "MyProject\\Team\\Sprint 1";
        assert_eq!(sanitize_config(input), input);
    }
}
