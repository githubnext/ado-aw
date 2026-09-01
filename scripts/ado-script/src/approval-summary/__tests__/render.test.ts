import { describe, it, expect } from "vitest";

import {
  BODY_MAX_CHARS,
  parseProposals,
  renderSummary,
  sanitizeBlock,
  sanitizeInline,
  type Proposal,
  type TrustedRepositoryContext,
} from "../render.js";

function ndjson(...records: Record<string, unknown>[]): string {
  return records.map((r) => JSON.stringify(r)).join("\n") + "\n";
}

function repositoryContext(
  tool: string,
  targetRepo = "octo-org/octo-repo",
  allowedRepos: string[] = [],
): TrustedRepositoryContext {
  return {
    policies: new Map([
      [tool, { targetRepo, allowedRepos }],
    ]),
    currentRepository: "octo-org/current",
    currentProvider: "GitHub",
    githubApiUrl: "https://api.github.com",
  };
}

function repositoryRows(markdown: string): string[] {
  return markdown
    .split("\n")
    .filter((line) => line.startsWith("| Repository |"));
}

function linkRepositoryContext(): TrustedRepositoryContext {
  const context = repositoryContext("create-github-issue");
  context.policies = new Map([
    [
      "create-github-issue",
      {
        targetRepo: "octo/default",
        allowedRepos: ["octo/alternate"],
      },
    ],
    [
      "link-github-sub-issue",
      {
        targetRepo: "octo/default",
        allowedRepos: ["octo/alternate"],
      },
    ],
  ]);
  return context;
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
    // `<`/`>` are HTML-entity-encoded (renderer-agnostic), not backslash-escaped.
    expect(out).toContain("&lt;img&gt;");
    expect(out).not.toContain("\\<img");
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

  it("entity-encodes & so agent-supplied entities are shown literally", () => {
    const out = sanitizeInline("Tom &amp; Jerry &lt;tag&gt;");
    // The ampersands are encoded, so a browser cannot decode `&lt;` back to `<`.
    expect(out).toContain("&amp;amp;");
    expect(out).toContain("&amp;lt;");
    expect(out).not.toMatch(/&lt;tag/);
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

  it("surfaces work-item temporary IDs and assignment details", () => {
    const md = renderSummary(
      parseProposals(
        ndjson(
          {
            name: "create-work-item",
            title: "Investigate failure",
            description: "A detailed description for the new work item.",
            temporary_id: "#aw_bug1",
          },
          {
            name: "assign-work-item",
            work_item_id: "#aw_bug1",
            assignee: "owner@example.com",
          },
        ),
      ),
      new Set(["create-work-item", "assign-work-item"]),
    );
    expect(md).toContain("| Temporary ID | \\#aw\\_bug1 |");
    expect(md).toContain("| Work item | \\#aw\\_bug1 |");
    expect(md).toContain("| Assignee | owner@example.com |");
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

  it("renders markers for present empty values but omits absent fields", () => {
    const md = renderSummary(
      parseProposals(
        ndjson({
          name: "update-github-issue",
          issue_number: 42,
          title: "",
          labels: [],
          assignees: {},
          body: "",
        }),
      ),
      new Set(),
      repositoryContext("update-github-issue"),
    );
    expect(md).toContain("| Title | &lt;empty string&gt; |");
    expect(md).toContain("| Labels | &lt;empty array&gt; |");
    expect(md).toContain("| Assignees | &lt;empty object&gt; |");
    expect(md).toContain("&lt;empty string&gt;");
    expect(md).not.toContain("| Status |");
    expect(md).not.toContain("| Milestone |");
  });

  it("renders empty arrays and objects for generic tools", () => {
    const md = renderSummary(
      parseProposals(
        ndjson({ name: "future-tool", empty_array: [], empty_object: {} }),
      ),
      new Set(),
    );
    expect(md).toContain("| empty\\_array | &lt;empty array&gt; |");
    expect(md).toContain("| empty\\_object | &lt;empty object&gt; |");
  });

  it("surfaces diagnostic-tool free-text in a fenced body", () => {
    const md = renderSummary(
      parseProposals(
        ndjson(
          { name: "noop", context: "Nothing to do.\nAll inputs were valid." },
          { name: "report-incomplete", reason: "Ran out of API quota." },
          { name: "missing-tool", tool_name: "kubectl", context: "needed for deploy" },
          { name: "missing-data", data_type: "schema", reason: "not provided" },
        ),
      ),
      new Set(),
    );
    expect(md).toContain("`noop`");
    expect(md).toContain("```text");
    // noop's multi-line context goes in a fenced body, not a truncated cell.
    expect(md).toContain("Nothing to do.");
    expect(md).toContain("All inputs were valid.");
    // report-incomplete surfaces its reason.
    expect(md).toContain("`report-incomplete`");
    expect(md).toContain("Ran out of API quota.");
    // missing-tool shows the tool field + context body.
    expect(md).toContain("| Tool | kubectl |");
    // missing-data shows the data-type field + reason body.
    expect(md).toContain("| Data type | schema |");
  });

  it.each([
    {
      name: "create-github-issue",
      record: {
        title: "New issue",
        repository: "octo-org/octo-repo",
        temporary_id: "#aw_new1",
        body: "Issue body",
      },
      expected: [
        "Create GitHub issue",
        "| Temporary ID | \\#aw\\_new1 |",
        "Issue body",
      ],
    },
    {
      name: "set-github-issue-type",
      record: {
        issue_number: "#aw_new1",
        issue_type: "Bug",
        repository: "octo-org/octo-repo",
      },
      expected: ["Set GitHub issue type", "| Type | Bug |"],
    },
    {
      name: "comment-on-github-issue",
      record: {
        issue_number: 42,
        repository: "octo-org/octo-repo",
        body: "Status update",
      },
      expected: ["Comment on GitHub issue", "| Issue | 42 |", "Status update"],
    },
    {
      name: "hide-github-issue-comment",
      record: {
        comment_id: 99,
        reason: "OUTDATED",
        repository: "octo-org/octo-repo",
      },
      expected: ["Hide GitHub issue comment", "| Reason | OUTDATED |"],
    },
    {
      name: "add-github-issue-labels",
      record: {
        issue_number: 42,
        labels: ["bug", "triage"],
        repository: "octo-org/octo-repo",
      },
      expected: ["Add GitHub issue labels", "| Labels | bug, triage |"],
    },
    {
      name: "remove-github-issue-labels",
      record: {
        issue_number: 42,
        labels: ["stale"],
        repository: "octo-org/octo-repo",
      },
      expected: ["Remove GitHub issue labels", "| Labels | stale |"],
    },
    {
      name: "close-github-issue",
      record: {
        issue_number: 42,
        state_reason: "duplicate",
        duplicate_of: 7,
        repository: "octo-org/octo-repo",
        body: "Closing note",
      },
      expected: ["Close GitHub issue", "| Duplicate of | 7 |", "Closing note"],
    },
    {
      name: "update-github-issue",
      record: {
        issue_number: 42,
        status: "closed",
        operation: "replace-island",
        repository: "octo-org/octo-repo",
        body: "Updated status block",
      },
      expected: [
        "Update GitHub issue",
        "| Operation | replace\\-island |",
        "Updated status block",
      ],
    },
    {
      name: "set-github-issue-field",
      record: {
        issue_number: 42,
        field_name: "Priority",
        value: "High",
        repository: "octo-org/octo-repo",
      },
      expected: [
        "Set GitHub issue field",
        "| Field | Priority |",
        "| Value | High |",
      ],
    },
    {
      name: "assign-github-issue-milestone",
      record: {
        issue_number: 42,
        milestone_title: "v1",
        repository: "octo-org/octo-repo",
      },
      expected: ["Assign GitHub issue milestone", "| Milestone | v1 |"],
    },
    {
      name: "assign-github-issue-to-user",
      record: {
        issue_number: 42,
        assignees: ["octocat", "hubot"],
        repository: "octo-org/octo-repo",
      },
      expected: ["Assign GitHub issue to user", "| Assignees | octocat, hubot |"],
    },
    {
      name: "unassign-github-issue-from-user",
      record: {
        issue_number: 42,
        assignee: "octocat",
        repository: "octo-org/octo-repo",
      },
      expected: ["Unassign GitHub issue from user", "| Assignee | octocat |"],
    },
    {
      name: "link-github-sub-issue",
      record: {
        parent_issue_number: 42,
        sub_issue_number: "#aw_sub1",
        repository: "octo-org/octo-repo",
      },
      expected: [
        "Link GitHub sub\\-issue",
        "| Parent issue | 42 |",
        "| Sub\\-issue | \\#aw\\_sub1 |",
      ],
    },
  ])("renders tailored details for $name", ({ name, record, expected }) => {
    const md = renderSummary(
      parseProposals(ndjson({ name, ...record })),
      new Set(),
      repositoryContext(name),
    );
    expect(md).toContain(`\`${name}\``);
    for (const value of expected) expect(md).toContain(value);
  });
});

describe("renderSummary — security", () => {
  it("does not let a crafted tool name break the heading code span", () => {
    const md = renderSummary(
      parseProposals(ndjson({ name: "foo\nbar`baz", title: "x" })),
      new Set(),
    );
    const heading = md.split("\n").find((l) => l.startsWith("#### "));
    expect(heading).toBeDefined();
    // Newline and backtick stripped from the name → it renders as a single
    // clean code span on one line (if a newline survived, the heading would be
    // split across lines and this exact span would not appear).
    expect(heading).toContain("`foobarbaz`");
  });

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
    // The tag is HTML-entity-encoded (renderer-agnostic), so no raw `<`/`>`.
    expect(titleRow).toContain("&lt;script&gt;");
    expect(titleRow).not.toMatch(/<script>/);
    // The fenced body must not contain a raw ``` that breaks the block.
    const bodyStart = md.indexOf("```text");
    const after = md.slice(bodyStart + "```text".length);
    const closeFence = after.indexOf("```");
    // The only ``` after the opening fence is the intended closing fence —
    // the hostile ``` was neutralised.
    expect(after.slice(0, closeFence)).not.toContain("```");
  });

  it("never renders a hostile agent repository as the effective repository", () => {
    const hostile = "x | <script>alert(1)</script>\n```\n## Approved";
    const md = renderSummary(
      parseProposals(
        ndjson({
          name: "comment-on-github-issue",
          issue_number: hostile,
          repository: hostile,
          body: hostile,
        }),
      ),
      new Set(["comment-on-github-issue"]),
      repositoryContext("comment-on-github-issue"),
    );
    const issueRow = md.split("\n").find((line) => line.startsWith("| Issue |"));
    const repositoryRow = md
      .split("\n")
      .find((line) => line.startsWith("| Repository |"));
    expect(issueRow).toContain("\\|");
    expect(issueRow).toContain("&lt;script&gt;");
    expect(repositoryRow).toContain(
      "&lt;unresolved: requested repository is outside operator policy&gt;",
    );
    expect(repositoryRow).not.toContain("script");
    const bodyStart = md.indexOf("```text");
    const after = md.slice(bodyStart + "```text".length);
    const closeFence = after.indexOf("```");
    expect(after.slice(0, closeFence)).not.toContain("```");
  });

  it("uses target-repo when the proposal omits repository", () => {
    const md = renderSummary(
      parseProposals(
        ndjson({ name: "create-github-issue", title: "Trusted target" }),
      ),
      new Set(),
      repositoryContext("create-github-issue", "trusted/default"),
    );
    expect(md).toContain("| Repository | trusted/default |");
  });

  it("uses the trusted current GitHub repository when target-repo is absent", () => {
    const context = repositoryContext("create-github-issue");
    context.policies = new Map([
      ["create-github-issue", { allowedRepos: [] }],
    ]);
    const md = renderSummary(
      parseProposals(
        ndjson({ name: "create-github-issue", title: "Current target" }),
      ),
      new Set(),
      context,
    );
    expect(md).toContain("| Repository | octo\\-org/current |");
  });

  it("reports current-repository fallback as unresolved for non-GitHub sources", () => {
    const context = repositoryContext("create-github-issue");
    context.policies = new Map([
      ["create-github-issue", { allowedRepos: [] }],
    ]);
    context.currentProvider = "TfsGit";
    const md = renderSummary(
      parseProposals(
        ndjson({ name: "create-github-issue", title: "No target" }),
      ),
      new Set(),
      context,
    );
    expect(md).toContain(
      "&lt;unresolved: configure target\\-repo for this source&gt;",
    );
    expect(md).not.toContain("octo\\-org/current");
  });

  it("uses a preceding create proposal's allowed alternate repository for temporary-ID consumers", () => {
    const context = repositoryContext("create-github-issue");
    context.policies = new Map([
      [
        "create-github-issue",
        {
          targetRepo: "octo/default",
          allowedRepos: ["octo/alternate"],
        },
      ],
      [
        "set-github-issue-type",
        {
          targetRepo: "octo/default",
          allowedRepos: ["octo/alternate"],
        },
      ],
    ]);
    const md = renderSummary(
      parseProposals(
        ndjson(
          {
            name: "create-github-issue",
            title: "Alternate repository",
            repository: "octo/alternate",
            temporary_id: "#aw_alt1",
          },
          {
            name: "set-github-issue-type",
            issue_number: "#aw_alt1",
            issue_type: "Bug",
          },
        ),
      ),
      new Set(["set-github-issue-type"]),
      context,
    );
    expect(repositoryRows(md)).toEqual([
      "| Repository | octo/alternate |",
      "| Repository | octo/alternate |",
    ]);
  });

  it("maps temporary-ID consumers to the preceding create proposal's default target", () => {
    const context = repositoryContext("create-github-issue");
    context.policies = new Map([
      [
        "create-github-issue",
        { targetRepo: "octo/default", allowedRepos: [] },
      ],
      [
        "comment-on-github-issue",
        { targetRepo: "octo/default", allowedRepos: [] },
      ],
    ]);
    const md = renderSummary(
      parseProposals(
        ndjson(
          {
            name: "create-github-issue",
            title: "Default repository",
            temporary_id: "#aw_def1",
          },
          {
            name: "comment-on-github-issue",
            issue_number: "#aw_def1",
            body: "A follow-up comment",
          },
        ),
      ),
      new Set(),
      context,
    );
    expect(repositoryRows(md)).toEqual([
      "| Repository | octo/default |",
      "| Repository | octo/default |",
    ]);
  });

  it("does not resolve a temporary repository from a later create proposal", () => {
    const context = repositoryContext("create-github-issue");
    context.policies = new Map([
      [
        "create-github-issue",
        { targetRepo: "octo/default", allowedRepos: [] },
      ],
      [
        "set-github-issue-type",
        { targetRepo: "octo/default", allowedRepos: [] },
      ],
    ]);
    const md = renderSummary(
      parseProposals(
        ndjson(
          {
            name: "set-github-issue-type",
            issue_number: "#aw_late1",
            issue_type: "Bug",
          },
          {
            name: "create-github-issue",
            title: "Created too late",
            temporary_id: "#aw_late1",
          },
        ),
      ),
      new Set(),
      context,
    );
    expect(repositoryRows(md)).toEqual([
      "| Repository | &lt;unresolved: temporary repository not established by a preceding create\\-github\\-issue&gt; |",
      "| Repository | octo/default |",
    ]);
  });

  it("does not let hostile create data establish a temporary repository", () => {
    const hostile = "octo/evil | <script>alert(1)</script>";
    const context = repositoryContext("create-github-issue");
    context.policies = new Map([
      [
        "create-github-issue",
        {
          targetRepo: "octo/default",
          allowedRepos: ["octo/alternate"],
        },
      ],
      [
        "comment-on-github-issue",
        {
          targetRepo: "octo/default",
          allowedRepos: ["octo/alternate"],
        },
      ],
    ]);
    const md = renderSummary(
      parseProposals(
        ndjson(
          {
            name: "create-github-issue",
            title: "Hostile repository",
            repository: hostile,
            temporary_id: "#aw_bad1",
          },
          {
            name: "comment-on-github-issue",
            issue_number: "#aw_bad1",
            body: "Follow-up",
          },
        ),
      ),
      new Set(),
      context,
    );
    expect(repositoryRows(md)).toEqual([
      "| Repository | &lt;unresolved: requested repository is outside operator policy&gt; |",
      "| Repository | &lt;unresolved: temporary repository not established by a preceding create\\-github\\-issue&gt; |",
    ]);
    expect(repositoryRows(md).join("\n")).not.toContain("script");
    expect(repositoryRows(md).join("\n")).not.toContain("octo/evil");
  });

  it("renders hostile temporary-like references as explicitly unresolved", () => {
    const context = repositoryContext(
      "set-github-issue-type",
      "octo/default",
    );
    const md = renderSummary(
      parseProposals(
        ndjson({
          name: "set-github-issue-type",
          issue_number: "#aw_bad<script>",
          issue_type: "Bug",
        }),
      ),
      new Set(),
      context,
    );
    expect(repositoryRows(md)).toEqual([
      "| Repository | &lt;unresolved: invalid temporary issue reference&gt; |",
    ]);
    expect(md).not.toContain("| Repository | octo/default |");
  });

  it.each([
    {
      direction: "parent temporary and child numeric",
      parent_issue_number: "#aw_mix1",
      sub_issue_number: 42,
    },
    {
      direction: "parent numeric and child temporary",
      parent_issue_number: 42,
      sub_issue_number: "#aw_mix1",
    },
  ])(
    "resolves a matching mixed sub-issue link with $direction",
    ({ parent_issue_number, sub_issue_number }) => {
      const md = renderSummary(
        parseProposals(
          ndjson(
            {
              name: "create-github-issue",
              title: "Default temporary issue",
              temporary_id: "#aw_mix1",
            },
            {
              name: "link-github-sub-issue",
              parent_issue_number,
              sub_issue_number,
            },
          ),
        ),
        new Set(),
        linkRepositoryContext(),
      );
      expect(repositoryRows(md)).toEqual([
        "| Repository | octo/default |",
        "| Repository | octo/default |",
      ]);
    },
  );

  it.each([
    {
      direction: "parent temporary and child numeric",
      parent_issue_number: "#aw_mix2",
      sub_issue_number: 42,
    },
    {
      direction: "parent numeric and child temporary",
      parent_issue_number: 42,
      sub_issue_number: "#aw_mix2",
    },
  ])(
    "marks a mismatched mixed sub-issue link unresolved with $direction",
    ({ parent_issue_number, sub_issue_number }) => {
      const md = renderSummary(
        parseProposals(
          ndjson(
            {
              name: "create-github-issue",
              title: "Alternate temporary issue",
              repository: "octo/alternate",
              temporary_id: "#aw_mix2",
            },
            {
              name: "link-github-sub-issue",
              parent_issue_number,
              sub_issue_number,
            },
          ),
        ),
        new Set(),
        linkRepositoryContext(),
      );
      expect(repositoryRows(md)).toEqual([
        "| Repository | octo/alternate |",
        "| Repository | &lt;unresolved: numeric and temporary references resolve to different repositories&gt; |",
      ]);
    },
  );

  it.each([
    {
      direction: "parent temporary and child quoted numeric",
      parent_issue_number: "#aw_quote1",
      sub_issue_number: "42",
    },
    {
      direction: "parent quoted numeric and child temporary",
      parent_issue_number: "42",
      sub_issue_number: "#aw_quote1",
    },
  ])(
    "resolves a matching quoted-numeric mixed link with $direction",
    ({ parent_issue_number, sub_issue_number }) => {
      const md = renderSummary(
        parseProposals(
          ndjson(
            {
              name: "create-github-issue",
              title: "Default quoted-number issue",
              temporary_id: "#aw_quote1",
            },
            {
              name: "link-github-sub-issue",
              parent_issue_number,
              sub_issue_number,
            },
          ),
        ),
        new Set(),
        linkRepositoryContext(),
      );
      expect(repositoryRows(md)).toEqual([
        "| Repository | octo/default |",
        "| Repository | octo/default |",
      ]);
    },
  );

  it.each([
    {
      direction: "parent temporary and child quoted numeric",
      parent_issue_number: "#aw_quote2",
      sub_issue_number: "42",
    },
    {
      direction: "parent quoted numeric and child temporary",
      parent_issue_number: "42",
      sub_issue_number: "#aw_quote2",
    },
  ])(
    "marks a mismatched quoted-numeric mixed link unresolved with $direction",
    ({ parent_issue_number, sub_issue_number }) => {
      const md = renderSummary(
        parseProposals(
          ndjson(
            {
              name: "create-github-issue",
              title: "Alternate quoted-number issue",
              repository: "octo/alternate",
              temporary_id: "#aw_quote2",
            },
            {
              name: "link-github-sub-issue",
              parent_issue_number,
              sub_issue_number,
            },
          ),
        ),
        new Set(),
        linkRepositoryContext(),
      );
      expect(repositoryRows(md)).toEqual([
        "| Repository | octo/alternate |",
        "| Repository | &lt;unresolved: numeric and temporary references resolve to different repositories&gt; |",
      ]);
    },
  );

  it.each(["0", "-1", "42x", "18446744073709551616"])(
    "rejects invalid quoted numeric link reference %s",
    (parent_issue_number) => {
      const md = renderSummary(
        parseProposals(
          ndjson({
            name: "link-github-sub-issue",
            parent_issue_number,
            sub_issue_number: "#aw_quote3",
          }),
        ),
        new Set(),
        linkRepositoryContext(),
      );
      expect(repositoryRows(md)).toEqual([
        "| Repository | &lt;unresolved: invalid sub\\-issue link reference&gt; |",
      ]);
    },
  );

  it("uses the trusted policy repository for a two-numeric sub-issue link", () => {
    const md = renderSummary(
      parseProposals(
        ndjson({
          name: "link-github-sub-issue",
          parent_issue_number: 41,
          sub_issue_number: 42,
        }),
      ),
      new Set(),
      linkRepositoryContext(),
    );
    expect(repositoryRows(md)).toEqual([
      "| Repository | octo/default |",
    ]);
  });

  it("resolves two temporary sub-issue references in the same repository", () => {
    const md = renderSummary(
      parseProposals(
        ndjson(
          {
            name: "create-github-issue",
            title: "First issue",
            temporary_id: "#aw_same1",
          },
          {
            name: "create-github-issue",
            title: "Second issue",
            temporary_id: "#aw_same2",
          },
          {
            name: "link-github-sub-issue",
            parent_issue_number: "#aw_same1",
            sub_issue_number: "#aw_same2",
          },
        ),
      ),
      new Set(),
      linkRepositoryContext(),
    );
    expect(repositoryRows(md)).toEqual([
      "| Repository | octo/default |",
      "| Repository | octo/default |",
      "| Repository | octo/default |",
    ]);
  });

  it("marks two temporary sub-issue references in different repositories unresolved", () => {
    const md = renderSummary(
      parseProposals(
        ndjson(
          {
            name: "create-github-issue",
            title: "Default issue",
            temporary_id: "#aw_diff1",
          },
          {
            name: "create-github-issue",
            title: "Alternate issue",
            repository: "octo/alternate",
            temporary_id: "#aw_diff2",
          },
          {
            name: "link-github-sub-issue",
            parent_issue_number: "#aw_diff1",
            sub_issue_number: "#aw_diff2",
          },
        ),
      ),
      new Set(),
      linkRepositoryContext(),
    );
    expect(repositoryRows(md)).toEqual([
      "| Repository | octo/default |",
      "| Repository | octo/alternate |",
      "| Repository | &lt;unresolved: temporary references resolve to different repositories&gt; |",
    ]);
  });

  it.each([
    {
      label: "hostile reference",
      parent_issue_number: "<script>alert(1)</script>",
      expected: "&lt;unresolved: invalid sub\\-issue link reference&gt;",
    },
    {
      label: "missing preceding creation",
      parent_issue_number: "#aw_none1",
      expected:
        "&lt;unresolved: temporary repository not established by a preceding create\\-github\\-issue&gt;",
    },
  ])(
    "renders an explicitly unresolved repository for a $label",
    ({ parent_issue_number, expected }) => {
      const md = renderSummary(
        parseProposals(
          ndjson({
            name: "link-github-sub-issue",
            parent_issue_number,
            sub_issue_number: 42,
          }),
        ),
        new Set(),
        linkRepositoryContext(),
      );
      expect(repositoryRows(md)).toEqual([
        `| Repository | ${expected} |`,
      ]);
      expect(repositoryRows(md).join("\n")).not.toContain("script");
    },
  );
});
