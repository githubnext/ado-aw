import { describe, it, expect, afterEach } from "vitest";
import { mkdtempSync, readFileSync, writeFileSync, rmSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { main, parseReviewed } from "../index.js";

const dirs: string[] = [];
function freshDir(): string {
  const d = mkdtempSync(join(tmpdir(), "approval-summary-"));
  dirs.push(d);
  return d;
}

afterEach(() => {
  for (const d of dirs.splice(0)) rmSync(d, { recursive: true, force: true });
});

describe("parseReviewed", () => {
  it("splits a newline-delimited list, trims, and drops empties", () => {
    const set = parseReviewed(" create-pull-request \n \n add-pr-comment ");
    expect([...set].sort()).toEqual(["add-pr-comment", "create-pull-request"]);
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
