import { describe, expect, it } from "vitest";
import { stripFrontMatter } from "../frontmatter.js";

describe("stripFrontMatter", () => {
  it("strips a basic front matter block", () => {
    const src = "---\nname: foo\n---\nbody line\n";
    expect(stripFrontMatter(src)).toBe("body line\n");
  });

  it("returns content unchanged when no front matter", () => {
    expect(stripFrontMatter("hello\nworld\n")).toBe("hello\nworld\n");
  });

  it("handles CRLF line endings", () => {
    const src = "---\r\nname: foo\r\n---\r\nbody\r\n";
    expect(stripFrontMatter(src)).toBe("body\r\n");
  });

  it("strips a UTF-8 BOM and the front matter", () => {
    const src = "\uFEFF---\nname: foo\n---\nbody\n";
    expect(stripFrontMatter(src)).toBe("body\n");
  });

  it("throws on unterminated front matter", () => {
    expect(() => stripFrontMatter("---\nname: foo\n")).toThrow(
      /Unterminated/,
    );
  });

  it("returns empty string for front-matter-only input", () => {
    expect(stripFrontMatter("---\nname: foo\n---\n")).toBe("");
  });

  it("preserves multiple body paragraphs", () => {
    const src = "---\nname: x\n---\nfirst\n\nsecond paragraph\n";
    expect(stripFrontMatter(src)).toBe("first\n\nsecond paragraph\n");
  });

  it("does not strip --- lines that appear later in the body", () => {
    const src = "no front matter here\n---\nstill body\n";
    // No opening fence at byte 0 → return unchanged.
    expect(stripFrontMatter(src)).toBe(src);
  });
});
