import { describe, it, expect, afterEach } from "vitest";
import { mkdtempSync, readFileSync, writeFileSync, rmSync, existsSync } from "node:fs";
import { join } from "node:path";

import { main, parsePrPolicies, parseRepositoryPolicies, parseReviewed } from "../index.js";

const dirs: string[] = [];
function freshDir(): string {
  const d = mkdtempSync(join(process.cwd(), ".approval-summary-test-"));
  dirs.push(d);
  return d;
}

afterEach(() => {
  for (const d of dirs.splice(0)) rmSync(d, { recursive: true, force: true });
});

describe("parseReviewed", () => {
  it("only accepts compiler-normalized PR policies with lossless decimal fixed IDs", () => {
    const policies = parsePrPolicies(JSON.stringify({
      "update-pull-request": {target:{kind:"fixed",id:"18446744073709551615"}},
      "abandon-pull-request": {target:{kind:"triggering"}},
      "add-pull-request-labels": {target:{kind:"explicit"}},
      "bad-raw-target": {target:"42"},
      "bad-rounded-target": {target:{kind:"fixed",id:18446744073709552000}},
    }));
    expect(policies.size).toBe(3);
    expect(policies.get("update-pull-request")?.target).toEqual({kind:"fixed",id:"18446744073709551615"});
  });
  it("splits a newline-delimited list, trims, and drops empties", () => {
    const set = parseReviewed(" create-pull-request \n \n add-pull-request-comment ");
    expect([...set].sort()).toEqual(["add-pull-request-comment", "create-pull-request"]);
  });

  describe("parseRepositoryPolicies", () => {
    it("accepts compiler policy JSON and ignores malformed entries", () => {
      const policies = parseRepositoryPolicies(
        JSON.stringify({
          "create-github-issue": {
            targetRepo: "octo/default",
            allowedRepos: ["octo/other", 7],
          },
          bad: "agent text",
        }),
      );
      expect(policies.get("create-github-issue")).toEqual({
        targetRepo: "octo/default",
        allowedRepos: ["octo/other"],
      });
      expect(policies.has("bad")).toBe(false);
    });

    it("fails closed for invalid JSON", () => {
      expect(parseRepositoryPolicies("{ hostile").size).toBe(0);
    });
  });

  it("does not split on commas (a comma may appear in a YAML map key)", () => {
    const set = parseReviewed("weird,tool-name");
    expect([...set]).toEqual(["weird,tool-name"]);
  });

  it("returns an empty set for undefined/empty", () => {
    expect(parseReviewed(undefined).size).toBe(0);
    expect(parseReviewed("").size).toBe(0);
  });
});

describe("main", () => {
  it("previews native and synthetic triggering destinations without using self or fork metadata", () => {
    const directory = freshDir();
    const input = join(directory, "proposals.ndjson");
    const output = join(directory, "summary.md");
    writeFileSync(input, '{"name":"update-pull-request","title":"New title"}');
    const identity = {
      collection_uri:"https://dev.azure.com/org/", project:"Other", repository_name:"target",
      repository_id:"11111111-1111-1111-1111-111111111111", id:"42",
    };
    const common = {
      AW_SAFE_OUTPUTS_NDJSON:input,AW_APPROVAL_SUMMARY_OUT:output,
      AW_PR_POLICIES:JSON.stringify({"update-pull-request":{target:{kind:"triggering"}}}),
      ADO_AW_SELF_REPOSITORY_NAME:"templates",
      SYSTEM_PULLREQUEST_SOURCEREPOSITORYURI:"https://dev.azure.com/fork/Elsewhere/_git/source",
    };
    for (const env of [
      {...common,ADO_AW_TRIGGERING_PR_IDENTITY:JSON.stringify(identity)},
      {...common,ADO_AW_TRIGGERING_PR_CAPTURED:"true",ADO_AW_TRIGGER_COLLECTION_URI:identity.collection_uri,
        ADO_AW_TRIGGER_REPOSITORY_URI:"https://dev.azure.com/org/Other/_git/target",
        ADO_AW_TRIGGER_REPOSITORY_ID:identity.repository_id,ADO_AW_TRIGGER_REPOSITORY_PROVIDER:"TfsGit",
        ADO_AW_TRIGGER_BUILD_REASON:"PullRequest",ADO_AW_TRIGGER_PR_ID:"42"},
    ]) {
      expect(main(env)).toBe(0);
      const summary = readFileSync(output,"utf8");
      expect(summary).toContain("| PR | 42 |");
      expect(summary).toContain("https://dev.azure.com/org/Other/target");
      expect(summary).not.toContain("templates");
      expect(summary).not.toContain("/fork/");
    }
    main({...common,ADO_AW_TRIGGERING_PR_IDENTITY:"",SYSTEM_PULLREQUEST_PULLREQUESTID:"42"});
    expect(readFileSync(output,"utf8")).toContain("complete triggering PR identity unavailable");
  });

  it("writes a summary and returns 0 when proposals exist", () => {
    const dir = freshDir();
    const ndjsonPath = join(dir, "safe_outputs.ndjson");
    const outPath = join(dir, "ado-aw-safe-outputs.md");
    writeFileSync(
      ndjsonPath,
      JSON.stringify({ name: "create-pull-request", title: "T" }) + "\n",
      "utf8",
    );
    const rc = main({
      AW_SAFE_OUTPUTS_NDJSON: ndjsonPath,
      AW_APPROVAL_SUMMARY_OUT: outPath,
      AW_REVIEWED_TOOLS: "create-pull-request",
    } as NodeJS.ProcessEnv);
    expect(rc).toBe(0);
    expect(existsSync(outPath)).toBe(true);
    expect(readFileSync(outPath, "utf8")).toContain("Pending approval (1)");
  });

  it("is a no-op (exit 0, no file) when the proposals file is missing", () => {
    const dir = freshDir();
    const outPath = join(dir, "ado-aw-safe-outputs.md");
    const rc = main({
      AW_SAFE_OUTPUTS_NDJSON: join(dir, "does-not-exist.ndjson"),
      AW_APPROVAL_SUMMARY_OUT: outPath,
    } as NodeJS.ProcessEnv);
    expect(rc).toBe(0);
    expect(existsSync(outPath)).toBe(false);
  });

  it("is a no-op when the proposals file has no valid records", () => {
    const dir = freshDir();
    const ndjsonPath = join(dir, "safe_outputs.ndjson");
    const outPath = join(dir, "ado-aw-safe-outputs.md");
    writeFileSync(ndjsonPath, "\n\nnot json\n", "utf8");
    const rc = main({
      AW_SAFE_OUTPUTS_NDJSON: ndjsonPath,
      AW_APPROVAL_SUMMARY_OUT: outPath,
    } as NodeJS.ProcessEnv);
    expect(rc).toBe(0);
    expect(existsSync(outPath)).toBe(false);
  });

  it("returns 0 without writing when required env is missing", () => {
    const rc = main({} as NodeJS.ProcessEnv);
    expect(rc).toBe(0);
  });
});
