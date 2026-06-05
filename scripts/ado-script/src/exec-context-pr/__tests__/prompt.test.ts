import { describe, expect, it } from "vitest";

import { failureFragment, successFragment } from "../prompt.js";
import type { Identifiers } from "../validate.js";

const ids: Identifiers = {
  prId: "4242",
  project: "MyProject",
  repo: "my-repo",
  targetBranch: "refs/heads/main",
  targetShort: "main",
};

describe("successFragment", () => {
  it("interpolates identifiers into the header", () => {
    const out = successFragment(ids);
    expect(out).toContain("This is PR #4242 in project 'MyProject' / repository 'my-repo'.");
  });

  it("includes the 6 git inspection commands", () => {
    const out = successFragment(ids);
    expect(out).toContain("git diff --stat $BASE..$HEAD");
    expect(out).toContain("git diff --name-status $BASE..$HEAD");
    expect(out).toContain("git diff $BASE..$HEAD");
    expect(out).toContain("git diff $BASE..$HEAD -- <path>");
    expect(out).toContain("git show $HEAD:<path>");
    expect(out).toContain("git log  $BASE..$HEAD");
  });

  it("includes the BASE/HEAD shell-var setup lines", () => {
    const out = successFragment(ids);
    expect(out).toContain("BASE=$(cat aw-context/pr/base.sha)");
    expect(out).toContain("HEAD=$(cat aw-context/pr/head.sha)");
  });

  it("includes the 3 ADO MCP example calls with identifiers pre-filled", () => {
    const out = successFragment(ids);
    expect(out).toContain("repo_get_pull_request_by_id(project='MyProject', repositoryId='my-repo', pullRequestId=4242)");
    expect(out).toContain("repo_list_pull_request_threads(project='MyProject', repositoryId='my-repo', pullRequestId=4242)");
    expect(out).toContain("repo_create_pull_request_thread(project='MyProject', repositoryId='my-repo', pullRequestId=4242");
  });

  it("starts with the ## PR context header", () => {
    const out = successFragment(ids);
    expect(out).toContain("## PR context");
  });
});

describe("failureFragment", () => {
  it("includes the reason verbatim", () => {
    const out = failureFragment("Test failure reason.", { prId: "42", project: "P", repo: "R" });
    expect(out).toContain("Reason: Test failure reason.");
  });

  it("interpolates identifiers when present", () => {
    const out = failureFragment("oops", { prId: "42", project: "P", repo: "R" });
    expect(out).toContain("PR #42 in project P / repository R");
  });

  it("uses <unknown> placeholders for missing identifiers", () => {
    const out = failureFragment("oops", {});
    expect(out).toContain("PR #<unknown> in project <unknown> / repository <unknown>");
  });

  it("uses <unknown> for empty-string identifiers (not just undefined)", () => {
    const out = failureFragment("oops", { prId: "", project: "", repo: "" });
    expect(out).toContain("PR #<unknown> in project <unknown> / repository <unknown>");
  });

  it("includes guidance to surface failure and not produce empty review", () => {
    const out = failureFragment("oops", {});
    expect(out).toContain("Do NOT produce an empty review");
    expect(out).toContain("surface the failure and stop");
  });

  it("starts with the ## PR context header", () => {
    const out = failureFragment("oops", {});
    expect(out).toContain("## PR context");
  });
});
