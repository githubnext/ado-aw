import { afterEach, describe, expect, it } from "vitest";
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { parse } from "yaml";
import { parsePrPolicies } from "../index.js";
import { parseProposals, renderSummary } from "../render.js";

// Match the executor-E2E offline contract: build the local compiler first.
const binary = process.env.ADO_AW_BIN
  ?? resolve(process.cwd(), "..", "..", "target", "debug", process.platform === "win32" ? "ado-aw.exe" : "ado-aw");
const directories: string[] = [];
afterEach(() => {
  for (const directory of directories.splice(0)) rmSync(directory, { recursive: true, force: true });
});

function findSummaryEnv(value: unknown): Record<string, string> | undefined {
  if (!value || typeof value !== "object") return undefined;
  if (!Array.isArray(value)) {
    const object = value as Record<string, unknown>;
    if (object.displayName === "Render safe-outputs summary") return object.env as Record<string, string>;
  }
  for (const child of Object.values(value)) {
    const found = findSummaryEnv(child);
    if (found) return found;
  }
  return undefined;
}

describe.skipIf(!existsSync(binary))("compiler-to-preview target policy contract", () => {
  it("normalizes quoted/numeric fixed IDs and full-u64 targets before rendering", () => {
    const directory = mkdtempSync(join(process.cwd(), ".approval-policy-contract-"));
    directories.push(directory);
    execFileSync("git", ["init", "--quiet", directory]);
    execFileSync("git", ["-C", directory, "remote", "add", "origin", "https://dev.azure.com/org/Project/_git/policy"]);
    for (const id of ["42", "18446744073709551615"]) {
      const rendered: string[] = [];
      for (const target of [id, `"${id}"`]) {
        const source = join(directory, "policy.md");
        const output = join(directory, "policy.lock.yml");
        writeFileSync(source, [
          "---", "name: preview-contract", "description: Test", "target: standalone",
          "safe-outputs:", "  update-pull-request:", `    target: ${target}`,
          "  abandon-pull-request:", `    target: ${target}`,
          "  add-pull-request-labels:", "---", "Review the pull request.", "",
        ].join("\n"));
        execFileSync(binary, ["compile", source, "--output", output, "--force"], {
          cwd: directory, env: { ...process.env, CI: "true" }, stdio: "pipe",
        });
        const env = findSummaryEnv(parse(readFileSync(output, "utf8")));
        expect(env).toBeDefined();
        const policies = parsePrPolicies(env!.AW_PR_POLICIES);
        expect(policies.get("update-pull-request")?.target).toEqual({kind:"fixed",id});
        expect(policies.get("abandon-pull-request")?.target).toEqual({kind:"fixed",id});
        expect(policies.get("add-pull-request-labels")?.target).toEqual({kind:"explicit"});
        const summary = renderSummary(parseProposals('{"name":"update-pull-request","title":"New"}'), new Set(), {
          policies:new Map(),prPolicies:policies,
          triggeringPr:{collection_uri:"https://dev.azure.com/org/",project:"Project",repository_name:"policy",
            repository_id:"11111111-1111-1111-1111-111111111111",id:"7"},
        });
        expect(summary).toContain(`| PR | ${id} |`);
        rendered.push(summary);
      }
      expect(rendered[0]).toBe(rendered[1]);
    }
  }, 60_000);
});
