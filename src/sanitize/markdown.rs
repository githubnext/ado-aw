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
//!
//! # Assumption
//!
//! Content inside a protected range is copied through **verbatim**, so this
//! module assumes Azure DevOps agrees with [`pulldown_cmark`] about where a
//! code span, code fence or autolink starts and ends. If the two ever disagree,
//! markup inside what this module believes is code reaches the renderer
//! unfiltered. Everything that is *not* code — including a link destination
//! containing `<` or `>` — is deliberately routed through the allowlist so the
//! assumption stays confined to code.
//!
//! # Known rendering differences
//!
//! Cleaning normalises HTML, so a few inputs come back rendering the same but
//! written differently: `\r\n` becomes `\n`, void elements are rewritten
//! (`<br />` → `<br>`), an implied `<tbody>` is inserted into a table, a `>` in
//! the middle of a line is stored as `&gt;`, the leading newline inside a
//! `<pre>` is dropped, and the content of a raw-text element the allowlist
//! removes (such as `<noscript>`) is escaped rather than kept as markup.

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

/// Element names a parser really would treat as a tag.
///
/// This is *not* a permission list — [`ALLOWED_TAGS`] is. It only decides
/// whether `<name …>` is markup (hand it to the allowlist, which drops or
/// cleans it) or prose (escape it, so `Vec<String>` survives). It therefore has
/// to include the dangerous names too: escaping `<svg onload=alert(1)>` would
/// turn a dropped payload into visible text.
const KNOWN_TAGS: &[&str] = &[
    // HTML
    "a",
    "abbr",
    "acronym",
    "address",
    "applet",
    "area",
    "article",
    "aside",
    "audio",
    "b",
    "base",
    "basefont",
    "bdi",
    "bdo",
    "bgsound",
    "big",
    "blink",
    "blockquote",
    "body",
    "br",
    "button",
    "canvas",
    "caption",
    "center",
    "cite",
    "code",
    "col",
    "colgroup",
    "data",
    "datalist",
    "dd",
    "del",
    "details",
    "dfn",
    "dialog",
    "dir",
    "div",
    "dl",
    "dt",
    "em",
    "embed",
    "fieldset",
    "figcaption",
    "figure",
    "font",
    "footer",
    "form",
    "frame",
    "frameset",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "head",
    "header",
    "hgroup",
    "hr",
    "html",
    "i",
    "iframe",
    "image",
    "img",
    "input",
    "ins",
    "isindex",
    "kbd",
    "keygen",
    "label",
    "legend",
    "li",
    "link",
    "listing",
    "main",
    "map",
    "mark",
    "marquee",
    "menu",
    "menuitem",
    "meta",
    "meter",
    "nav",
    "nobr",
    "noembed",
    "noframes",
    "noscript",
    "object",
    "ol",
    "optgroup",
    "option",
    "output",
    "p",
    "param",
    "picture",
    "plaintext",
    "pre",
    "progress",
    "q",
    "rb",
    "rp",
    "rt",
    "rtc",
    "ruby",
    "s",
    "samp",
    "script",
    "search",
    "section",
    "select",
    "slot",
    "small",
    "source",
    "spacer",
    "span",
    "strike",
    "strong",
    "style",
    "sub",
    "summary",
    "sup",
    "table",
    "tbody",
    "td",
    "template",
    "textarea",
    "tfoot",
    "th",
    "thead",
    "time",
    "title",
    "tr",
    "track",
    "tt",
    "u",
    "ul",
    "var",
    "video",
    "wbr",
    "xmp",
    // SVG and MathML names that browsers parse as foreign markup
    "animate",
    "animatemotion",
    "animatetransform",
    "circle",
    "clippath",
    "defs",
    "desc",
    "ellipse",
    "feimage",
    "filter",
    "foreignobject",
    "g",
    "line",
    "linegradient",
    "maction",
    "malignmark",
    "math",
    "menclose",
    "merror",
    "mfenced",
    "mfrac",
    "mglyph",
    "mi",
    "mn",
    "mo",
    "mpath",
    "mroot",
    "mrow",
    "ms",
    "mspace",
    "msqrt",
    "mstyle",
    "msub",
    "msubsup",
    "msup",
    "mtable",
    "mtd",
    "mtext",
    "mtr",
    "munder",
    "munderover",
    "path",
    "polygon",
    "polyline",
    "rect",
    "semantics",
    "set",
    "svg",
    "switch",
    "symbol",
    "text",
    "textpath",
    "tspan",
    "use",
];

/// Valueless attributes a real tag can carry. No attribute the allowlist keeps
/// is valueless, so anything else without a value means the `<` was prose.
const BOOLEAN_ATTRIBUTES: &[&str] = &[
    "allowfullscreen",
    "async",
    "autofocus",
    "autoplay",
    "checked",
    "controls",
    "default",
    "defer",
    "disabled",
    "hidden",
    "inert",
    "ismap",
    "itemscope",
    "loop",
    "multiple",
    "muted",
    "nomodule",
    "novalidate",
    "open",
    "playsinline",
    "readonly",
    "required",
    "reversed",
    "selected",
];

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

/// Private-use characters that stand in for content the HTML allowlist must not
/// see. They are stripped from the input first, so nothing an author writes can
/// forge one.
const PLACEHOLDER_OPEN: char = '\u{E000}';
const PLACEHOLDER_CLOSE: char = '\u{E001}';
/// Stands in for a line-leading blockquote marker while the allowlist runs, so
/// a `>` the author wrote as markup survives and a `&gt;` the author wrote as
/// text is never promoted to markup.
const BLOCKQUOTE_MARKER: char = '\u{E002}';

const SENTINELS: [char; 3] = [PLACEHOLDER_OPEN, PLACEHOLDER_CLOSE, BLOCKQUOTE_MARKER];

/// Apply the HTML allowlist and URL scheme policy to `input`, leaving Markdown
/// code spans, code fences and autolinks untouched.
///
/// `transform_text` is applied to every non-protected source range before the
/// HTML allowlist runs, so caller-owned text transformations (mentions, bot
/// triggers) also skip code.
///
/// Protected and redacted ranges are swapped for placeholders and the document
/// is cleaned in a **single** pass, so inline HTML that spans a code span
/// (`<b>bold `code` more</b>`) is not rebalanced around the code span.
pub(super) fn sanitize_markdown_html(
    input: &str,
    mut transform_text: impl FnMut(&str) -> String,
) -> String {
    let input = strip_sentinels(input);
    let input = input.as_ref();

    let mut assembled = String::with_capacity(input.len());
    let mut verbatim: Vec<&str> = Vec::new();
    let mut cursor = 0;

    for region in regions(input) {
        let range = region.range();
        if range.start < cursor {
            // Overlapping regions can only happen for nested constructs; the
            // outer one already covered this range.
            continue;
        }
        assembled.push_str(&transform_text(&input[cursor..range.start]));
        let text = match region {
            Region::Protected(_) => &input[range.clone()],
            Region::Redact(_) => REDACTED,
        };
        assembled.push(PLACEHOLDER_OPEN);
        assembled.push_str(&verbatim.len().to_string());
        assembled.push(PLACEHOLDER_CLOSE);
        verbatim.push(text);
        cursor = range.end;
    }
    assembled.push_str(&transform_text(&input[cursor..]));

    let assembled = escape_non_tag_markup(&assembled);
    let assembled = mark_blockquotes(&assembled);

    restore(&clean_html(&assembled), &verbatim)
}

fn strip_sentinels(input: &str) -> std::borrow::Cow<'_, str> {
    if input.contains(SENTINELS) {
        std::borrow::Cow::Owned(input.replace(SENTINELS, ""))
    } else {
        std::borrow::Cow::Borrowed(input)
    }
}

/// Replace every line-leading blockquote marker with [`BLOCKQUOTE_MARKER`].
///
/// The allowlist escapes `>` in text, and an escaped marker at the start of a
/// line no longer opens a blockquote. Recording the markers *before* cleaning
/// is what keeps the restore honest: only a `>` the author actually wrote as
/// markup comes back.
///
/// Runs after [`escape_non_tag_markup`], so every remaining `<` really does
/// open a tag: a `>` that closes a tag written across two lines is left alone
/// instead of being mistaken for a blockquote marker.
fn mark_blockquotes(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut in_tag = false;

    for line in input.split_inclusive('\n') {
        let mut rest = line;
        if !in_tag {
            loop {
                let indent = rest.len() - rest.trim_start_matches(' ').len();
                if indent > 3 {
                    break;
                }
                let Some(after) = rest[indent..].strip_prefix('>') else {
                    break;
                };
                result.push_str(&rest[..indent]);
                result.push(BLOCKQUOTE_MARKER);
                rest = after;
            }
        }

        for character in rest.chars() {
            match character {
                '<' => in_tag = true,
                '>' => in_tag = false,
                _ => {}
            }
        }
        result.push_str(rest);
    }

    result
}

/// Escape every `<` that does not start something an HTML parser would treat as
/// a tag, so text that merely looks like markup survives as text.
///
/// The allowlist *deletes* markup it does not recognise, which would silently
/// eat `Vec<String>` out of a description. Escaping first means unknown markup
/// degrades to `&lt;`, while anything a browser really would parse as a tag
/// still reaches the allowlist and is dropped or cleaned there.
fn escape_non_tag_markup(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut rest = input;

    while let Some(offset) = rest.find('<') {
        result.push_str(&rest[..offset]);
        if starts_html_tag(&rest[offset + 1..]) {
            result.push('<');
        } else {
            result.push_str("&lt;");
        }
        rest = &rest[offset + 1..];
    }
    result.push_str(rest);
    result
}

/// How far past a `<` the tag test looks. Every `<` in the document is tested,
/// so an unbounded scan would be quadratic on input made of nothing but `<`.
/// A start tag longer than this is treated as markup and handed to the
/// allowlist, which is the same thing that happens to an unterminated tag.
const TAG_SCAN_LIMIT: usize = 256;

/// Whether the text following a `<` is markup rather than prose.
///
/// True for comments, doctypes and processing instructions (all removed by the
/// allowlist), and for a start or end tag whose name is a real HTML, SVG or
/// MathML element written with plausible attribute syntax. Bare attributes are
/// the discriminator that keeps `if a<b and b>c` prose: no allowed attribute is
/// valueless, so a valueless attribute that is not a known boolean attribute
/// means the author was not writing a tag.
fn starts_html_tag(rest: &str) -> bool {
    let rest = bounded(rest);
    if rest.starts_with('!') || rest.starts_with('?') {
        return true;
    }

    let (closing, rest) = match rest.strip_prefix('/') {
        Some(rest) => (true, rest),
        None => (false, rest),
    };

    let name_len = rest
        .find(|c: char| c.is_whitespace() || c == '/' || c == '>')
        .unwrap_or(rest.len());
    let name = &rest[..name_len];
    if !name
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        || !KNOWN_TAGS.contains(&name.to_ascii_lowercase().as_str())
    {
        return false;
    }

    let rest = &rest[name_len..];
    if closing {
        // `</b>`; anything else after the name is not how an end tag is written.
        return rest.trim_start().starts_with('>') || rest.trim().is_empty();
    }
    plausible_attributes(rest)
}

/// The first [`TAG_SCAN_LIMIT`] bytes of `input`, cut on a character boundary.
fn bounded(input: &str) -> &str {
    let mut end = input.len().min(TAG_SCAN_LIMIT);
    while !input.is_char_boundary(end) {
        end -= 1;
    }
    &input[..end]
}

/// Whether the remainder of a start tag is written as plausible attributes.
///
/// An unterminated tag (`<script src=x`) is left to the allowlist, which drops
/// it — escaping it instead would resurrect the payload as visible text.
fn plausible_attributes(rest: &str) -> bool {
    let mut rest = rest.trim_start();

    loop {
        rest = rest.trim_start_matches(['/', ' ', '\t', '\n', '\r']);
        if rest.is_empty() {
            return true;
        }
        if rest.starts_with('>') {
            return true;
        }

        let name_len = rest
            .find(|c: char| c.is_whitespace() || c == '=' || c == '/' || c == '>')
            .unwrap_or(rest.len());
        let name = &rest[..name_len];
        if name.is_empty() {
            return false;
        }
        rest = rest[name_len..].trim_start();

        let Some(value) = rest.strip_prefix('=') else {
            if !BOOLEAN_ATTRIBUTES.contains(&name.to_ascii_lowercase().as_str()) {
                return false;
            }
            continue;
        };

        let value = value.trim_start();
        rest = match value.chars().next() {
            Some(quote @ ('"' | '\'')) => match value[1..].find(quote) {
                Some(end) => &value[1 + end + 1..],
                // An unterminated quoted value swallows the rest of the input;
                // the allowlist drops the whole tag.
                None => return true,
            },
            _ => {
                let end = value
                    .find(|c: char| c.is_whitespace() || c == '>')
                    .unwrap_or(value.len());
                &value[end..]
            }
        };
    }
}

/// Put back everything the allowlist was not allowed to see, and undo the one
/// piece of text escaping that changes how Markdown renders.
fn restore(cleaned: &str, verbatim: &[&str]) -> String {
    let restored = unescape_ampersands(cleaned).replace(BLOCKQUOTE_MARKER, ">");

    let mut result = String::with_capacity(restored.len());
    let mut rest = restored.as_str();

    while let Some(offset) = rest.find(PLACEHOLDER_OPEN) {
        result.push_str(&rest[..offset]);
        let after = &rest[offset + PLACEHOLDER_OPEN.len_utf8()..];
        let Some(end) = after.find(PLACEHOLDER_CLOSE) else {
            // The allowlist truncated the placeholder, so its content is gone
            // with the markup that contained it.
            rest = after;
            continue;
        };
        if let Some(text) = after[..end]
            .parse::<usize>()
            .ok()
            .and_then(|index| verbatim.get(index))
        {
            result.push_str(text);
        }
        rest = &after[end + PLACEHOLDER_CLOSE.len_utf8()..];
    }
    result.push_str(rest);
    result
}

/// Undo `&` escaping in text so prose (`R&D`) and bare URLs
/// (`https://example.test/x?a=1&b=2`, which Azure DevOps autolinks) keep the
/// character the author wrote.
///
/// `&lt;` must stay escaped — that is what stops a dropped tag being re-parsed
/// as markup — and escaping inside a tag is left alone, so an attribute value
/// can never be decoded into a different URL than the one the allowlist
/// approved.
fn unescape_ampersands(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut in_tag = false;

    let mut rest = input;
    while let Some(offset) = rest.find(['<', '>', '&']) {
        result.push_str(&rest[..offset]);
        match &rest[offset..offset + 1] {
            "<" => {
                in_tag = true;
                result.push('<');
                rest = &rest[offset + 1..];
            }
            ">" => {
                in_tag = false;
                result.push('>');
                rest = &rest[offset + 1..];
            }
            _ if !in_tag && rest[offset..].starts_with("&amp;") => {
                result.push('&');
                rest = &rest[offset + "&amp;".len()..];
            }
            _ => {
                result.push('&');
                rest = &rest[offset + 1..];
            }
        }
    }
    result.push_str(rest);
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
                    } else if let Some(destination) =
                        protectable_destination(input, &range, &dest_url, INLINE_LABEL_END)
                    {
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
                        // Only the destination is redacted, so nested elements
                        // (`[![alt](vbscript:inner)](vbscript:outer)`) keep
                        // being walked and have their own destinations checked.
                        // When the destination cannot be located the whole
                        // element is redacted instead, and the overlapping
                        // nested regions are dropped while rebuilding.
                        regions.push(Region::Redact(
                            locate_destination(input, &range, &dest_url, INLINE_LABEL_END)
                                .unwrap_or(range),
                        ));
                    }
                }
            }
            _ => {}
        }
    }

    for (_, definition) in iter.reference_definitions().iter() {
        if scheme_allowed(&definition.dest) {
            if let Some(range) = protectable_destination(
                input,
                &definition.span,
                &definition.dest,
                DEFINITION_LABEL_END,
            ) {
                regions.push(Region::Protected(range));
            }
        } else if let Some(range) = locate_destination(
            input,
            &definition.span,
            &definition.dest,
            DEFINITION_LABEL_END,
        ) {
            regions.push(Region::Redact(range));
        }
    }

    regions.sort_by_key(|region| region.range().start);
    regions
}

/// Label terminator that a destination follows in `[label](dest)` links and
/// `![alt](src)` images.
const INLINE_LABEL_END: &str = "](";
/// Label terminator that a destination follows in a `[label]: dest` reference
/// definition.
const DEFINITION_LABEL_END: &str = "]:";

/// Find a destination that may be copied through verbatim.
///
/// Protected ranges bypass the HTML allowlist, so a destination is only
/// eligible when it cannot itself carry markup: a destination containing `<`
/// or `>` falls through to the allowlist, which escapes it. That keeps the
/// module's guarantee (nothing outside the tag allowlist reaches the output)
/// true even when the renderer's idea of where a destination ends differs from
/// [`pulldown_cmark`]'s.
///
/// A destination wrapped in pointy brackets (`[a](<https://example.test/a b>)`)
/// keeps the brackets inside the protected range, so a destination containing
/// spaces is not broken by escaping the delimiters.
fn protectable_destination(
    input: &str,
    span: &Range<usize>,
    dest_url: &str,
    label_end: &str,
) -> Option<Range<usize>> {
    let range = locate_destination(input, span, dest_url, label_end)?;
    let destination = input.get(range.clone())?;
    if destination.contains('<') || destination.contains('>') {
        return None;
    }

    if input[..range.start].ends_with('<') && input[range.end..].starts_with('>') {
        return Some(range.start - 1..range.end + 1);
    }
    Some(range)
}

/// Find the literal destination text inside the element's source range.
///
/// The search starts *after* the label terminator, because in every syntax that
/// has a label the destination follows it. Searching the whole span instead
/// matches the label whenever it repeats the destination text — for
/// `[javascript:alert(1)](javascript:alert(1))` that redacts the inert label
/// and leaves the live destination untouched.
///
/// `dest_url` is the parser's decoded destination, so it does not always appear
/// verbatim in the source (percent/entity/backslash escapes). When it cannot be
/// located the caller degrades safely: a denied scheme redacts the whole
/// element, and an allowed destination is simply not marked protected, so it
/// flows through the HTML allowlist like ordinary text.
fn locate_destination(
    input: &str,
    span: &Range<usize>,
    dest_url: &str,
    label_end: &str,
) -> Option<Range<usize>> {
    if dest_url.is_empty() {
        return None;
    }
    let source = input.get(span.clone())?;
    // An autolink (`<https://example.test>`) has no label, so the whole span is
    // the destination and the search starts at the beginning.
    let search_from = source
        .rfind(label_end)
        .map_or(0, |offset| offset + label_end.len());
    let offset = search_from + source.get(search_from..)?.find(dest_url)?;
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
    CLEANER.clean(input).to_string()
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
    fn redacts_destination_when_the_label_repeats_it() {
        // The destination follows the label, so locating it must not stop at
        // the first matching bytes — otherwise the label is redacted and the
        // live destination survives.
        let output = sanitize_markdown("[javascript:alert(1)](javascript:alert(1))");

        assert_eq!(output, "[javascript:alert(1)]((redacted))", "{output}");
    }

    #[test]
    fn redacts_reference_definition_when_the_label_repeats_it() {
        let output = sanitize_markdown(
            "See [x][vbscript:msgbox(1)].\n\n[vbscript:msgbox(1)]: vbscript:msgbox(1)\n",
        );

        assert_eq!(
            output, "See [x][vbscript:msgbox(1)].\n\n[vbscript:msgbox(1)]: (redacted)\n",
            "{output}"
        );
    }

    #[test]
    fn protects_allowed_destination_when_the_label_repeats_it() {
        let input = "[https://example.test/@user](https://example.test/@user)";

        // The label is prose and gets mention neutralization; the destination
        // is a URL and must be left alone.
        assert_eq!(
            sanitize_markdown(input),
            "[https://example.test/`@user`](https://example.test/@user)"
        );
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
    fn redacts_nested_denied_destinations() {
        // The outer element is not skipped wholesale, so the inner image's
        // destination is checked too.
        assert_eq!(
            sanitize_markdown("[![x](vbscript:inner)](vbscript:outer)"),
            "[![x]((redacted))]((redacted))"
        );

        let output =
            sanitize_markdown("[![x](data:text/html,<script>alert(1)</script>)](javascript:o)");

        assert!(!output.contains("data:"), "{output}");
        assert!(!output.contains("<script"), "{output}");
    }

    #[test]
    fn cleans_markup_smuggled_into_an_allowed_destination() {
        // A destination carrying `<` or `>` is never copied through verbatim,
        // so the tag allowlist still sees every byte of it.
        let output = sanitize_markdown("[x](https://e.test/a<img/onerror=alert(1)>)");

        assert!(!output.contains("onerror"), "{output}");

        let output = sanitize_markdown("[r]: https://e.test/<script>\n");

        assert!(!output.contains("<script"), "{output}");
    }

    #[test]
    fn preserves_pointy_bracket_destination_with_spaces() {
        let input = "[a](<https://example.test/a b>)";
        assert_eq!(sanitize_markdown(input), input);
    }

    #[test]
    fn escapes_text_that_only_looks_like_a_tag() {
        // Unknown markup degrades to an entity instead of being deleted, so a
        // description keeps the author's text.
        assert_eq!(
            sanitize_markdown("Fix `Vec<String>` and Vec<String> and Map<K,V>"),
            "Fix `Vec<String>` and Vec&lt;String&gt; and Map&lt;K,V&gt;"
        );
        assert_eq!(sanitize_markdown("<tag>"), "&lt;tag&gt;");
        assert_eq!(
            sanitize_markdown("<www.example.test>"),
            "&lt;www.example.test&gt;"
        );
    }

    #[test]
    fn escapes_prose_that_parses_as_a_tag_with_bare_attributes() {
        // `<b and b>` is a valid start tag to an HTML parser, which would
        // delete the text between the angle brackets.
        assert_eq!(
            sanitize_markdown("if a<b and b>c then"),
            "if a&lt;b and b&gt;c then"
        );
    }

    #[test]
    fn preserves_boolean_attributes_on_real_tags() {
        // A valueless attribute that a tag really can carry stays markup; the
        // allowlist drops the attribute itself.
        assert_eq!(
            sanitize_markdown("<details open>text</details>"),
            "<details>text</details>"
        );
    }

    #[test]
    fn does_not_promote_author_escaped_text_to_a_blockquote() {
        let input = "&gt; not quoted\n";
        assert_eq!(sanitize_markdown(input), input);
    }

    #[test]
    fn does_not_steal_the_close_of_a_tag_written_across_lines() {
        // A `>` on its own line closes the tag above it; treating it as a
        // blockquote marker would leave an unterminated tag and delete the text
        // that follows.
        assert_eq!(sanitize_markdown("<b\n>text</b>\n"), "<b>text</b>\n");
    }

    #[test]
    fn preserves_inline_html_spanning_a_code_span() {
        // The document is cleaned in one pass, so the open tag is not closed at
        // the code span boundary.
        let input = "<b>bold `code` more</b>";
        assert_eq!(sanitize_markdown(input), input);
    }

    #[test]
    fn preserves_ampersands_in_prose_and_bare_urls() {
        let input = "https://example.test/x?a=1&b=2 and R&D";
        assert_eq!(sanitize_markdown(input), input);
    }

    #[test]
    fn keeps_attribute_entities_escaped() {
        // Decoding `&amp;` inside an attribute could turn an approved URL into
        // a denied one, so escaping is only undone in text.
        let input = r#"<a href="&amp;#106;avascript:alert(1)">x</a>"#;
        assert_eq!(sanitize_markdown(input), input);
    }

    #[test]
    fn tag_test_is_bounded_on_adversarial_input() {
        // Every `<` is tested, so the scan is capped: an input made of nothing
        // but `<` must stay linear rather than rescanning the document each
        // time.
        let start = std::time::Instant::now();
        let output = sanitize_markdown(&"<b x=1 ".repeat(50_000));

        assert!(!output.contains('<'), "unterminated tags are dropped");
        assert!(start.elapsed().as_secs() < 10, "{:?}", start.elapsed());
    }

    #[test]
    fn is_idempotent() {
        for input in [
            "[![x](vbscript:inner)](vbscript:outer)",
            "Fix `Vec<String>` and Vec<String>",
            "if a<b and b>c then",
            "> quote\n> > nested\n",
            "&gt; not quoted\n",
            "https://example.test/x?a=1&b=2 and R&D",
            "<b>bold `code` more</b>",
            &rendering_corpus::input(),
        ] {
            let once = sanitize_markdown(input);

            assert_eq!(sanitize_markdown(&once), once, "input: {input:?}");
        }
    }

    #[test]
    fn ignores_forged_internal_sentinels() {
        // The private-use characters that stand in for protected content are
        // stripped from the input, so they cannot be used to smuggle text past
        // the allowlist or to forge a blockquote.
        let output = sanitize_markdown("a\u{E000}0\u{E001}b\u{E002}> c");

        assert_eq!(output, "a0b&gt; c");
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
