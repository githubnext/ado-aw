import { describe, expect, it } from "vitest";

import { FrontMatterError, stripFrontMatter } from "../frontmatter.js";

describe("stripFrontMatter", () => {
  it("returns body unchanged when no front matter", () => {
    const src = "# Hello\n\nNo front matter here.";
    expect(stripFrontMatter(src)).toBe(src);
  });

  it("strips a simple front-matter block, preserving body whitespace", () => {
    // The close delimiter line (`---\n`) is consumed in full; any
    // blank lines between the close and the body are preserved as-is
    // (matches Rust's `body_raw` semantics — the runtime caller
    // applies its own `.trim()` before joining with supplements).
    const src = "---\nname: x\ndescription: y\n---\n\nBody text\n";
    expect(stripFrontMatter(src)).toBe("\nBody text\n");
  });

  it("preserves leading whitespace in the body", () => {
    // Regression for the same kind of bug that fooled the inlined
    // branch's `trim_start_matches(' ')`: stripping the front matter
    // must NOT eat author-supplied leading whitespace in the body.
    const src = "---\nname: x\ndescription: y\n---\n    indented opening\nplain";
    expect(stripFrontMatter(src)).toBe("    indented opening\nplain");
  });

  it("supports CRLF line endings", () => {
    const src = "---\r\nname: x\r\n---\r\n\r\nBody\r\n";
    expect(stripFrontMatter(src)).toBe("\r\nBody\r\n");
  });

  it("throws when front matter is opened but never closed", () => {
    const src = "---\nname: x\nno closing delim here";
    expect(() => stripFrontMatter(src)).toThrow(FrontMatterError);
  });

  it("does not treat mid-body --- as a close", () => {
    // The body itself contains `---` (e.g. a horizontal rule). That
    // first `---` should still be the close delimiter, because Rust
    // semantics close on the FIRST `---` line after the open. Any
    // blank line between the close and the first body content is
    // preserved as a leading newline.
    const src = "---\nname: x\n---\n\nIntro\n\n---\n\nRest\n";
    expect(stripFrontMatter(src)).toBe("\nIntro\n\n---\n\nRest\n");
  });

  it("returns empty string when body is empty", () => {
    const src = "---\nname: x\n---\n";
    expect(stripFrontMatter(src)).toBe("");
  });

  it("handles front matter immediately followed by content", () => {
    // No blank line between `---` and the first content line.
    const src = "---\nname: x\n---\nFirst line";
    expect(stripFrontMatter(src)).toBe("First line");
  });
});
