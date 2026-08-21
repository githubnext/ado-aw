//! Markdown-aware sanitization for untrusted agent content that Azure DevOps
//! renders as Markdown (for example a work item description field stored with
//! `multilineFieldsFormat: Markdown`).
//!
//! The policy has two halves:
//!
//! * **Markdown structure** — [`pulldown_cmark`] parses the content so the
//!   sanitizer knows which source ranges are code spans, code fences and
//!   autolinks (left verbatim), and which ranges are link/image destinations
//!   (checked against a URL scheme allowlist).
//! * **Inline HTML** — everything outside those protected ranges goes through
//!   an [`ammonia`] allowlist. Azure DevOps only renders a small subset of HTML
//!   inside Markdown fields (and explicitly blocks scripting constructs such as
//!   `<script>` and `<iframe>`), so the allowlist starts from the tags that
//!   have a Markdown equivalent instead of trying to enumerate dangerous tags.
//!
//! Enumerating *dangerous* tags is what the previous hand-rolled implementation
//! did; every new evasion (folded tags, unquoted attributes, decoded schemes)
//! needed another special case. Allowlisting is closed by construction: anything
//! the parser does not recognise as an allowed tag is dropped, and anything that
//! is not an allowed URL scheme is removed.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::LazyLock;

use ammonia::{Builder, UrlRelative};
use pulldown_cmark::{Event, LinkType, Options, Parser, Tag};

/// Placeholder substituted for a destination whose scheme is not allowed.
const REDACTED: &str = "(redacted)";

/// HTML tags preserved inside a Markdown description.
///
/// Restricted to formatting elements that have a Markdown equivalent, so a
/// renderer that ignores raw HTML still shows equivalent content.
const ALLOWED_TAGS: &[&str] = &[
    "a",
    "b",
    "blockquote",
    "br",
    "code",
    "dd",
    "del",
    "details",
    "div",
    "dl",
    "dt",
    "em",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "i",
    "img",
    "ins",
    "kbd",
    "li",
    "ol",
    "p",
    "pre",
    "s",
    "samp",
    "span",
    "strong",
    "sub",
    "summary",
    "sup",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "tr",
    "u",
    "ul",
];

/// URL schemes allowed in HTML attributes and in Markdown link/image
/// destinations. Everything else (`javascript:`, `data:`, `file:`,
/// `vbscript:`, …) is denied because it is not on this list.
const ALLOWED_URL_SCHEMES: &[&str] = &["http", "https", "mailto"];

static CLEANER: LazyLock<Builder<'static>> = LazyLock::new(|| {
    let mut builder = Builder::empty();
    builder
        .tags(ALLOWED_TAGS.iter().copied().collect())
        .clean_content_tags(["script", "style"].into_iter().collect())
        .generic_attributes(["title"].into_iter().collect())
        .tag_attributes(tag_attributes())
        .url_schemes(ALLOWED_URL_SCHEMES.iter().copied().collect())
        .url_relative(UrlRelative::PassThrough)
        .link_rel(None)
        .strip_comments(true);
    builder
});

fn tag_attributes() -> HashMap<&'static str, HashSet<&'static str>> {
    let mut attributes: HashMap<&'static str, HashSet<&'static str>> = HashMap::new();
    attributes.insert("a", ["href"].into_iter().collect());
    attributes.insert(
        "img",
        ["src", "alt", "width", "height"].into_iter().collect(),
    );
    for cell in ["td", "th"] {
        attributes.insert(cell, ["colspan", "rowspan", "align"].into_iter().collect());
    }
    attributes
}

/// Apply the HTML allowlist and URL scheme policy to `input`, leaving Markdown
/// code spans, code fences and autolinks untouched.
///
/// `transform_text` is applied to every non-protected source range before the
/// HTML allowlist runs, so caller-owned text transformations (mentions, bot
/// triggers) also skip code.
pub(super) fn sanitize_markdown_html(
    input: &str,
    mut transform_text: impl FnMut(&str) -> String,
) -> String {
    let mut result = String::with_capacity(input.len());
    let mut cursor = 0;

    for region in regions(input) {
        let range = region.range();
        if range.start < cursor {
            // Overlapping regions can only happen for nested constructs; the
            // outer one already covered this range.
            continue;
        }
        result.push_str(&clean_html(&transform_text(&input[cursor..range.start])));
        match region {
            Region::Protected(_) => result.push_str(&input[range.clone()]),
            Region::Redact(_) => result.push_str(REDACTED),
        }
        cursor = range.end;
    }

    result.push_str(&clean_html(&transform_text(&input[cursor..])));
    result
}

/// Source ranges of Markdown code spans, code fences and indented code blocks.
///
/// Exposed for the plain-text sanitizer, which escapes HTML tags everywhere
/// except inside code.
pub(super) fn code_ranges(input: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut skip_until = 0;

    for (event, range) in Parser::new_ext(input, options()).into_offset_iter() {
        if range.start < skip_until {
            continue;
        }
        match event {
            Event::Code(_) => ranges.push(range),
            Event::Start(Tag::CodeBlock(_)) => {
                skip_until = range.end;
                ranges.push(range);
            }
            _ => {}
        }
    }

    ranges
}

enum Region {
    /// Copied through verbatim.
    Protected(Range<usize>),
    /// Replaced with [`REDACTED`].
    Redact(Range<usize>),
}

impl Region {
    fn range(&self) -> Range<usize> {
        match self {
            Region::Protected(range) | Region::Redact(range) => range.clone(),
        }
    }
}

fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
}

fn regions(input: &str) -> Vec<Region> {
    let mut regions: Vec<Region> = Vec::new();
    let mut skip_until = 0;

    let mut iter = Parser::new_ext(input, options()).into_offset_iter();
    let events: Vec<_> = iter.by_ref().collect();

    for (event, range) in events {
        if range.start < skip_until {
            continue;
        }
        match event {
            Event::Code(_) => regions.push(Region::Protected(range)),
            Event::Start(Tag::CodeBlock(_)) => {
                skip_until = range.end;
                regions.push(Region::Protected(range));
            }
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                ..
            })
            | Event::Start(Tag::Image {
                link_type,
                dest_url,
                ..
            }) => {
                if scheme_allowed(&dest_url) {
                    if matches!(link_type, LinkType::Autolink | LinkType::Email) {
                        // `<https://example.test>` must not be rewritten by the
                        // HTML allowlist, which would parse it as a bogus tag.
                        skip_until = range.end;
                        regions.push(Region::Protected(range));
                    } else if let Some(destination) = locate_destination(input, &range, &dest_url) {
                        // A destination is a URL, not prose: text transforms
                        // such as @mention wrapping must not rewrite it.
                        regions.push(Region::Protected(destination));
                    }
                    continue;
                }

                match link_type {
                    // The destination lives in a reference definition, which is
                    // handled separately below.
                    LinkType::Reference
                    | LinkType::ReferenceUnknown
                    | LinkType::Collapsed
                    | LinkType::CollapsedUnknown
                    | LinkType::Shortcut
                    | LinkType::ShortcutUnknown => {}
                    _ => {
                        skip_until = range.end;
                        regions.push(Region::Redact(
                            locate_destination(input, &range, &dest_url).unwrap_or(range),
                        ));
                    }
                }
            }
            _ => {}
        }
    }

    for (_, definition) in iter.reference_definitions().iter() {
        let Some(range) = locate_destination(input, &definition.span, &definition.dest) else {
            continue;
        };
        if scheme_allowed(&definition.dest) {
            regions.push(Region::Protected(range));
        } else {
            regions.push(Region::Redact(range));
        }
    }

    regions.sort_by_key(|region| region.range().start);
    regions
}

/// Find the literal destination text inside the element's source range.
///
/// `dest_url` is the parser's decoded destination, so it does not always appear
/// verbatim in the source (percent/entity/backslash escapes). When it cannot be
/// located the caller degrades safely: a denied scheme redacts the whole
/// element, and an allowed destination is simply not marked protected, so it
/// flows through the HTML allowlist like ordinary text.
fn locate_destination(input: &str, span: &Range<usize>, dest_url: &str) -> Option<Range<usize>> {
    if dest_url.is_empty() {
        return None;
    }
    let source = input.get(span.clone())?;
    let offset = source.find(dest_url)?;
    let start = span.start + offset;
    Some(start..start + dest_url.len())
}

/// Deny-by-default scheme check: relative destinations and fragments are
/// allowed, absolute ones must use an allowed scheme.
fn scheme_allowed(dest_url: &str) -> bool {
    let trimmed = dest_url.trim();
    let Some(colon) = trimmed.find(':') else {
        return true;
    };
    let scheme = &trimmed[..colon];
    // A colon that appears after a path separator is not a scheme delimiter.
    if scheme.contains('/') || scheme.contains('?') || scheme.contains('#') {
        return true;
    }
    // The parser has already decoded entities, so an evasion like
    // `java&Tab;script:` arrives here with the whitespace in place.
    let scheme = scheme.trim().to_ascii_lowercase();
    ALLOWED_URL_SCHEMES.contains(&scheme.as_str())
}

fn clean_html(input: &str) -> String {
    if input.is_empty() {
        return String::new();
    }
    let cleaned = CLEANER.clean(input).to_string();
    restore_markdown_text(&cleaned)
}

/// Undo the one piece of HTML text escaping that changes how Markdown renders.
///
/// The serializer escapes `<`, `>` and `&` in text nodes. `&lt;` must stay
/// escaped — that is what stops a dropped tag from being re-parsed as markup —
/// and an escaped `&`/`>` in the middle of a line renders identically to the
/// raw character. A leading `>` is different: it opens a blockquote, so it is
/// restored at the start of every line.
fn restore_markdown_text(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    for line in input.split_inclusive('\n') {
        result.push_str(&restore_blockquote_markers(line));
    }
    result
}

fn restore_blockquote_markers(line: &str) -> String {
    let mut rest = line;
    let mut prefix = String::new();

    loop {
        let indent = rest.len() - rest.trim_start_matches(' ').len();
        if indent > 3 {
            break;
        }
        let Some(after) = rest[indent..].strip_prefix("&gt;") else {
            break;
        };
        prefix.push_str(&rest[..indent]);
        prefix.push('>');
        rest = after;
    }

    if prefix.is_empty() {
        return line.to_string();
    }
    prefix + rest
}

/// Shared work-item rendering-fidelity corpus.
///
/// The same JSON is imported by the `create-work-item-rendering` executor E2E
/// scenarios, so the fast local golden and the against-ADO assertion can never
/// disagree about what a human is supposed to see in a work item.
#[cfg(test)]
pub(crate) mod rendering_corpus {
    const CORPUS: &str = include_str!(
        "../../scripts/ado-script/src/executor-e2e/scenarios/markdown-rendering-corpus.json"
    );

    fn lines(key: &str) -> String {
        let corpus: serde_json::Value = serde_json::from_str(CORPUS).expect("corpus is valid JSON");
        corpus[key]
            .as_array()
            .unwrap_or_else(|| panic!("corpus key '{key}' is not an array"))
            .iter()
            .map(|line| line.as_str().expect("corpus line is not a string"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The unsanitized Markdown an agent proposes.
    pub(crate) fn input() -> String {
        lines("input")
    }

    /// The sanitized Markdown that must be stored in the work item.
    pub(crate) fn expected() -> String {
        lines("expected")
    }
}

#[cfg(test)]
mod tests {
    use super::rendering_corpus;
    use crate::sanitize::sanitize_markdown;

    #[test]
    fn preserves_plain_markdown_structure() {
        let input = "# Title\n\n- item one\n- item two\n\n> quoted line\n\n| a | b |\n| - | - |\n| 1 | 2 |\n\n**bold** and _italic_\n";
        assert_eq!(sanitize_markdown(input), input);
    }

    #[test]
    fn escapes_bare_less_than_in_prose() {
        // `<` stays escaped so a dropped tag can never be re-parsed as markup.
        // Markdown renders the entity as the original character.
        assert_eq!(sanitize_markdown("a < b"), "a &lt; b");
    }

    #[test]
    fn preserves_nested_blockquotes() {
        let input = "> outer\n> > inner\n";
        assert_eq!(sanitize_markdown(input), input);
    }

    #[test]
    fn preserves_markdown_links_with_allowed_schemes() {
        let input = "[docs](https://learn.microsoft.com/azure/devops) and [mail](mailto:team@example.test) and [rel](./page.md#frag)";
        assert_eq!(sanitize_markdown(input), input);
    }

    #[test]
    fn preserves_autolinks() {
        let input = "<https://dev.azure.com/org/project> and <team@example.test>";
        assert_eq!(sanitize_markdown(input), input);
    }

    #[test]
    fn redacts_markdown_link_destination_with_denied_scheme() {
        let output = sanitize_markdown("[click me](javascript:alert(1))");

        assert!(!output.contains("javascript:"), "{output}");
        assert!(output.contains("(redacted)"), "{output}");
        assert!(output.contains("click me"), "{output}");
    }

    #[test]
    fn redacts_markdown_image_destination_with_denied_scheme() {
        let output = sanitize_markdown("![logo](data:text/html;base64,PHNjcmlwdD4=)");

        assert!(!output.contains("data:"), "{output}");
        assert!(output.contains("(redacted)"), "{output}");
    }

    #[test]
    fn redacts_autolink_with_denied_scheme() {
        let output = sanitize_markdown("<javascript:alert(1)>");

        assert!(!output.contains("javascript:"), "{output}");
    }

    #[test]
    fn redacts_reference_definition_with_denied_scheme() {
        let output = sanitize_markdown("See [the link][ref].\n\n[ref]: vbscript:msgbox(1)\n");

        assert!(!output.contains("vbscript:"), "{output}");
        assert!(output.contains("(redacted)"), "{output}");
        assert!(output.contains("the link"), "{output}");
    }

    #[test]
    fn preserves_reference_definition_with_allowed_scheme() {
        let input = "See [the link][ref].\n\n[ref]: https://example.test/page\n";
        assert_eq!(sanitize_markdown(input), input);
    }

    #[test]
    fn strips_denied_scheme_in_html_attribute_regardless_of_encoding() {
        for href in [
            "javascript:alert(1)",
            "JaVaScRiPt:alert(1)",
            "&#106;avascript:alert(1)",
            "java\tscript:alert(1)",
            " javascript:alert(1)",
            "data:text/html,<h1>hi</h1>",
            "file:///etc/passwd",
            "vbscript:msgbox(1)",
        ] {
            let output = sanitize_markdown(&format!(r#"<a href="{href}">link</a>"#));

            assert_eq!(output, "<a>link</a>", "href: {href}");
        }
    }

    #[test]
    fn strips_denied_scheme_in_markdown_link_regardless_of_case() {
        let output = sanitize_markdown("[x](JAVASCRIPT:alert(1))");

        assert!(
            !output.to_ascii_lowercase().contains("javascript:"),
            "{output}"
        );
    }

    #[test]
    fn drops_svg_and_mathml_payloads() {
        let output = sanitize_markdown(
            "<svg><animate onbegin=alert(1) attributeName=x dur=1s></animate></svg>\
             <math><mtext><table><mglyph><style><img src=x onerror=alert(1)></style></mglyph></mtext></math>",
        );

        assert!(!output.contains("<svg"), "{output}");
        assert!(!output.contains("<math"), "{output}");
        assert!(!output.contains("onerror"), "{output}");
        assert!(!output.contains("onbegin"), "{output}");
        assert!(!output.contains("alert"), "{output}");
    }

    #[test]
    fn drops_unquoted_and_malformed_attributes() {
        let output =
            sanitize_markdown(r#"<a href=https://example.test onclick=alert(1) x=<b>>link</a>"#);

        assert!(!output.contains("onclick"), "{output}");
        assert!(!output.contains("alert"), "{output}");
        assert!(output.contains("link"), "{output}");
    }

    #[test]
    fn drops_disallowed_attributes_on_allowed_tags() {
        let output =
            sanitize_markdown(r#"<div id="x" class="y" style="color:red" title="ok">text</div>"#);

        assert_eq!(output, r#"<div title="ok">text</div>"#);
    }

    #[test]
    fn preserves_html_and_markdown_inside_inline_code_spans() {
        let input = "Use `<script>alert(1)</script>` and ``a `b` [x](javascript:1)`` inline.";
        assert_eq!(sanitize_markdown(input), input);
    }

    #[test]
    fn preserves_tilde_fenced_and_indented_code_blocks() {
        let input =
            "~~~\n<script>alert(1)</script>\n~~~\n\ntext\n\n    <iframe src=evil></iframe>\n";
        assert_eq!(sanitize_markdown(input), input);
    }

    #[test]
    fn skips_mention_and_bot_trigger_neutralization_inside_code() {
        let input = "```\n@user fixes #12 AB#34\n```\n";
        assert_eq!(sanitize_markdown(input), input);
    }

    #[test]
    fn neutralizes_mentions_and_bot_triggers_outside_code() {
        let output = sanitize_markdown("@user fixes #12 AB#34");

        assert!(output.contains("`@user`"), "{output}");
        assert!(output.contains("`fixes #12`"), "{output}");
        assert!(output.contains("`AB#34`"), "{output}");
    }

    #[test]
    fn neutralizes_pipeline_commands_inside_code_blocks() {
        // Pipeline command neutralization is a transport concern: the string is
        // echoed by the agent job, where a fence is not a fence.
        let output = sanitize_markdown("```\n##vso[task.setvariable variable=x]y\n```\n");

        assert!(!output.contains("##vso[task"), "{output}");
    }

    #[test]
    fn removes_html_comments_including_unclosed() {
        assert_eq!(sanitize_markdown("a<!-- <script>x</script> -->b"), "ab");
        assert_eq!(sanitize_markdown("a<!-- unterminated"), "a");
    }

    #[test]
    fn redacts_encoded_denied_scheme_in_markdown_link() {
        // The parser decodes the destination, so the literal source text does
        // not match; the whole element is redacted rather than passed through.
        let output = sanitize_markdown("[x](javascript&#58;alert&#40;1&#41;)");

        assert!(!output.contains("javascript"), "{output}");
        assert!(output.contains("(redacted)"), "{output}");
    }

    #[test]
    fn removes_control_characters() {
        let output = sanitize_markdown("a\x1b[31mb\x00c");

        assert_eq!(output, "abc");
    }

    #[test]
    fn work_item_rendering_corpus_matches_golden() {
        let actual = sanitize_markdown(&rendering_corpus::input());

        assert_eq!(
            actual,
            rendering_corpus::expected(),
            "sanitized rendering corpus changed; \
             update `expected` in markdown-rendering-corpus.json only when the \
             new rendering is intentional (it is asserted byte-for-byte against \
             a real work item by the executor E2E suite)"
        );
    }
}
