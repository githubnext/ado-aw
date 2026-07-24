import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";
import { parse } from "yaml";

const pipelinePath = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "../../../../../tests/compiler-smoke-e2e/azure-pipelines.yml",
);

describe("candidate compiler trigger policy", () => {
  const text = readFileSync(pipelinePath, "utf8");
  const pipeline = parse(text) as {
    trigger?: string;
    pr?: { branches?: { include?: string[] } };
    schedules?: Array<{
      cron?: string;
      branches?: { include?: string[] };
      always?: boolean;
    }>;
  };

  it("keeps PRs eligible for the Azure Pipelines comment trigger", () => {
    expect(pipeline.trigger).toBe("none");
    expect(pipeline.pr?.branches?.include).toEqual(["main"]);
  });

  it("runs the latest main candidate every day", () => {
    expect(pipeline.schedules).toEqual([
      expect.objectContaining({
        cron: "0 1 * * *",
        branches: { include: ["main"] },
        always: true,
      }),
    ]);
  });

  it("fails when the live orchestrator loses its all-PR comment gate", () => {
    for (const contract of [
      ".isCommentRequiredForPullRequest == true",
      ".requireCommentsForNonTeamMembersOnly == false",
      ".requireCommentsForNonTeamMemberAndNonContributors == false",
      ".isCommentRequiredForInternalRepoPRs == true",
      '.commentOptionInternalRepos == "all"',
    ]) {
      expect(text).toContain(contract);
    }
  });
});
