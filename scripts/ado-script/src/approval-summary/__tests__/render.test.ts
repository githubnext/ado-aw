import { describe, it, expect } from "vitest";

import {
  BODY_MAX_CHARS,
  parseProposals,
  renderSummary,
  sanitizeBlock,
  sanitizeInline,
  type Proposal,
} from "../render.js";

function ndjson(...records: Record<string, unknown>[]): string {
  return records.map((r) => JSON.stringify(r)).join("\n") + "\n";
}

describe("parseProposals", () => {
  it("parses one proposal per non-blank line with a string name", () => {
    const text = ndjson(
      { name: "create-pull-request", title: "T" },
      { name: "add-pr-comment", content: "C" },
    );
    const out = parseProposals(text);
    expect(out.map((p) => p.name)).toEqual([
      "create-pull-request",
      "add-pr-comment",
    ]);
    expect(out.map((p) => p.index)).toEqual([0, 1]);
  });

  it("skips blank lines, malformed JSON, non-objects, and records with no name", () => {
    const text = [
      "",
      "not json",
      JSON.stringify([1, 2, 3]),
      JSON.stringify({ noName: true }),
      JSON.stringify({ name: "" }),
      JSON.stringify({ name: "noop", context: "ok" }),
      "   ",
    ].join("\n");
    const out = parseProposals(text);
    expect(out).toHaveLength(1);
    expect(out[0]?.name).toBe("noop");
  });
});

describe("sanitizeInline", () => {
  it("escapes markdown/HTML/table metacharacters so content renders literally", () => {
    const out = sanitizeInline("**bold** [x](y) <img> | cell `code`");
    expect(out).not.toContain("**bold**");
    expect(out).toContain("\\*\\*bold\\*\\*");
    expect(out).toContain("\\|");
    expect(out).toContain("\\<img\\>");
    expect(out).toContain("\\[x\\]");
  });

  it("collapses to a single line and strips control characters", () => {
    const out = sanitizeInline("line1\nline2\tcol\u0000\u0007");
    expect(out).not.toMatch(/[\n\t\u0000\u0007]/);
    expect(out).toContain("line1 line2 col");
  });

  it("renders arrays as comma-joined values", () => {
    expect(sanitizeInline(["a", "b", "c"])).toBe("a, b, c");
  });

  it("truncates very long values", () => {
    const out = sanitizeInline("x".repeat(5000));
    expect(out.length).toBeLessThan(5000);
    expect(out).toContain("(truncated)");
  });
});

describe("sanitizeBlock", () => {
  it("neutralises embedded code fences so the body cannot escape the block", () => {
    const out = sanitizeBlock("before\n```\nbreakout\n```\nafter");
    expect(out).not.toContain("```");
    expect(out).toContain("breakout");
  });

  it("preserves newlines but strips other control characters", () => {
    const out = sanitizeBlock("a\nb\u0000\u0007c");
    expect(out).toContain("a\nb");
    expect(out).not.toMatch(/[\u0000\u0007]/);
  });

  it("truncates bodies longer than BODY_MAX_CHARS", () => {
    const out = sanitizeBlock("y".repeat(BODY_MAX_CHARS + 500));
    expect(out.length).toBeLessThan(BODY_MAX_CHARS + 500);
    expect(out).toContain("(truncated)");
  });
});

describe("renderSummary — grouping/ordering", () => {
  const proposals: Proposal[] = parseProposals(
    ndjson(
      { name: "add-pr-comment", pull_request_id: 5, content: "auto comment" },
      { name: "create-pull-request", title: "Reviewed PR", source_branch: "feat/x" },
      { name: "create-work-item", title: "Reviewed WI" },
    ),
  );

  it("lists pending-approval proposals BEFORE automatic ones", () => {
    const reviewed = new Set(["create-pull-request", "create-work-item"]);
    const md = renderSummary(proposals, reviewed);
    const pendingIdx = md.indexOf("Pending approval");
    const autoIdx = md.indexOf("Automatic");
    expect(pendingIdx).toBeGreaterThan(-1);
    expect(autoIdx).toBeGreaterThan(-1);
    expect(pendingIdx).toBeLessThan(autoIdx);
    // Reviewed tools appear in the pending section (before Automatic heading).
    const pendingBlock = md.slice(pendingIdx, autoIdx);
    expect(pendingBlock).toContain("create-pull-request");
    expect(pendingBlock).toContain("create-work-item");
    expect(pendingBlock).not.toContain("add-pr-comment");
  });

  it("counts the pending and automatic groups", () => {
    const reviewed = new Set(["create-pull-request", "create-work-item"]);
    const md = renderSummary(proposals, reviewed);
    expect(md).toContain("Pending approval (2)");
    expect(md).toContain("Automatic (1)");
  });

  it("renders a single 'All proposals' list when nothing is reviewed", () => {
    const md = renderSummary(proposals, new Set());
    expect(md).toContain("All proposals (3)");
    expect(md).not.toContain("Pending approval");
    expect(md).not.toContain("Automatic (");
  });

  it("returns an empty string for no proposals", () => {
    expect(renderSummary([], new Set())).toBe("");
  });
});

describe("renderSummary — per-tool detail", () => {
  it("uses tailored fields + a fenced body for a known tool", () => {
    const md = renderSummary(
      parseProposals(
        ndjson({
          name: "create-pull-request",
          title: "My PR",
          source_branch: "feat/x",
          repository: "self",
          description: "Body line one\nBody line two",
        }),
      ),
      new Set(),
    );
    expect(md).toContain("Create pull request");
    expect(md).toContain("| Title | My PR |");
    expect(md).toContain("| Source branch | feat/x |");
    expect(md).toContain("```text");
    expect(md).toContain("Body line one");
  });

  it("falls back to generic scalar fields for an unmapped tool", () => {
    const md = renderSummary(
      parseProposals(
        ndjson({ name: "future-tool", alpha: "a", zeta: 9, obj: { x: 1 } }),
      ),
      new Set(),
    );
    // Title-cased fallback heading.
    expect(md).toContain("Future tool");
    // Scalar fields surfaced in sorted order; nested object skipped.
    expect(md).toContain("| alpha | a |");
    expect(md).toContain("| zeta | 9 |");
    expect(md).not.toContain("obj");
  });
});

describe("renderSummary — security", () => {
  it("does not let agent content forge UI or break out of the layout", () => {
    const hostile =
      "Looks fine | ✅ APPROVED | <script>alert(1)</script>\n```\n## Fake heading";
    const md = renderSummary(
      parseProposals(
        ndjson({
          name: "create-pull-request",
          title: hostile,
          description: hostile,
        }),
      ),
      new Set(["create-pull-request"]),
    );
    // Inline title escaped: no raw pipe (would add a table column) or raw tag.
    const titleRow = md.split("\n").find((l) => l.startsWith("| Title |"));
    expect(titleRow).toBeDefined();
    expect(titleRow).toContain("\\|");
    expect(titleRow).toContain("\\<script\\>");
    // The fenced body must not contain a raw ``` that breaks the block.
    const bodyStart = md.indexOf("```text");
    const after = md.slice(bodyStart + "```text".length);
    const closeFence = after.indexOf("```");
    // The only ``` after the opening fence is the intended closing fence —
    // the hostile ``` was neutralised.
    expect(after.slice(0, closeFence)).not.toContain("```");
  });
});
