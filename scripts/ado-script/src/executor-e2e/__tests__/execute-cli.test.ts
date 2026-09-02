import { describe, expect, it } from "vitest";

import {
  parseExecutedRecords,
  renderNdjsonLine,
  renderSourceMarkdown,
} from "../execute-cli.js";

describe("renderSourceMarkdown", () => {
  it("emits front matter with inline-JSON safe-outputs config", () => {
    const md = renderSourceMarkdown({
      tool: "comment-on-work-item",
      safeOutputs: { "comment-on-work-item": { target: "*", max: 1 } },
    });
    expect(md).toContain('name: "executor-e2e: comment-on-work-item"');
    expect(md).toContain("target: standalone");
    expect(md).toContain("safe-outputs:");
    expect(md).toContain('"comment-on-work-item": {"target":"*","max":1}');
    expect(md).not.toContain("repos:");
    // Balanced front-matter fences.
    expect(md.match(/^---$/gm)?.length).toBe(2);
  });

  it("emits a repos block when adoRepo is provided", () => {
    const md = renderSourceMarkdown({
      tool: "add-pr-comment",
      safeOutputs: { "add-pr-comment": { "allowed-repositories": ["agent-definitions"] } },
      adoRepo: "agent-definitions",
    });
    expect(md).toContain("repos:");
    expect(md).toContain(`  - "agent-definitions=agent-definitions"`);
  });

  it("emits one safe-outputs key per tool when a scenario stages prior entries", () => {
    const md = renderSourceMarkdown({
      tool: "set-github-issue-type",
      safeOutputs: {
        "create-github-issue": { "target-repo": "o/r" },
        "set-github-issue-type": { "target-repo": "o/r" },
      },
    });
    expect(md).toContain('"create-github-issue": {"target-repo":"o/r"}');
    expect(md).toContain('"set-github-issue-type": {"target-repo":"o/r"}');
    expect(md.match(/^---$/gm)?.length).toBe(2);
  });

  it("emits expanded write permissions and cross-org repository metadata", () => {
    const md = renderSourceMarkdown({
      tool: "create-branch",
      safeOutputs: { "create-branch": { "allowed-repositories": ["target"] } },
      source: {
        repositories: [{
          name: "Other Project/target-repo",
          alias: "target",
          organization: "other-org",
          endpoint: "ado-write",
        }],
        writePermissions: {
          serviceConnection: "ado-write",
          connectionType: "azureDevOps",
          allow: [{
            organization: "other-org",
            projects: [{
              project: "Other Project",
              repositories: ["target-repo"],
            }],
          }],
        },
      },
    });

    expect(md).toContain("permissions:");
    expect(md).toContain('"connection-type":"azureDevOps"');
    expect(md).toContain(
      '  - {"name":"Other Project/target-repo","alias":"target","organization":"other-org","endpoint":"ado-write"}',
    );
  });
});

describe("renderNdjsonLine", () => {
  it("prepends the tool name and serialises one line", () => {
    const line = renderNdjsonLine("create-work-item", { title: "t", description: "d" });
    expect(line.endsWith("\n")).toBe(true);
    const parsed = JSON.parse(line);
    expect(parsed).toEqual({ name: "create-work-item", title: "t", description: "d" });
  });
});

describe("parseExecutedRecords", () => {
  it("parses valid records and ignores blank/malformed lines", () => {
    const content = [
      '{"name":"create_work_item","status":"succeeded","result":{"id":5}}',
      "",
      "not json",
      '{"missing":"fields"}',
      '{"name":"comment_on_work_item","status":"failed","error":"boom"}',
    ].join("\n");
    const records = parseExecutedRecords(content);
    expect(records).toHaveLength(2);
    expect(records[0]!.name).toBe("create_work_item");
    expect(records[0]!.result).toEqual({ id: 5 });
    expect(records[1]!.status).toBe("failed");
    expect(records[1]!.error).toBe("boom");
  });
});
