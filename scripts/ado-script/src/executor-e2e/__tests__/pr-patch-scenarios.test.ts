import { mkdtemp, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it, vi } from "vitest";
import { AdoRest } from "../ado-rest.js";
import type { ScenarioContext } from "../scenario.js";
import { createPullRequestScenarios, seedExecutableFixture } from "../scenarios/create-pull-request.js";
import { prPushScenarios } from "../scenarios/pr-push.js";

describe("native patch live coverage", () => {
  it("registers mandatory create and push fidelity cases", () => {
    expect(createPullRequestScenarios.map((scenario) => scenario.id)).toEqual(expect.arrayContaining(
      ["native-copy", "native-rename", "excluded-copy", "crlf", "binary", "expansion-denied"].map((mode) => `create-pull-request-${mode}`),
    ));
    expect(prPushScenarios.map((scenario) => scenario.id)).toEqual(expect.arrayContaining(
      ["native-copy", "native-rename", "excluded-native-copy", "crlf", "binary", "expansion-denied"].map((mode) => `pr-push-${mode}`),
    ));
    const denied = prPushScenarios.find((scenario) => scenario.id === "pr-push-expansion-denied")!;
    expect(denied.expectedFailure).toBeDefined();
    expect(denied.assertFailure).toBeDefined();
    for (const [prefix, scenarios] of [["create-pull-request", createPullRequestScenarios], ["pr-push", prPushScenarios]] as const) {
      for (const mode of ["space-edit", "space-rename-edit", "mode-up-denied", "mode-down-denied", "mode-new-denied", "mode-rename-denied", "mode-edit"]) {
        const scenario = scenarios.find((candidate) => candidate.id === `${prefix}-${mode}`);
        expect(scenario, `${prefix}-${mode}`).toBeDefined();
        if (mode.endsWith("-denied")) {
          expect(scenario?.expectedFailure).toBeDefined();
          expect(scenario?.assertFailure).toBeDefined();
        }
      }
    }
  });

  it("does not seed executable modes outside an unchanged owned fixture branch", async () => {
    const rest = new AdoRest({ orgUrl: "https://dev.azure.com/org", project: "project", token: "test" });
    const observed = vi.spyOn(rest, "getRefObjectId").mockResolvedValue("b".repeat(40));
    const context: ScenarioContext = {
      orgUrl: "https://dev.azure.com/org", project: "project", token: "test", rest,
      adoRepo: "repo", buildId: "42", adoAwBin: "unused", workDir: "unused",
      log: () => {}, prefix: (id) => `ado-aw-det-42-${id}`,
    };
    for (const branch of ["main", "ado-aw-det-42-fixture"]) {
      await expect(seedExecutableFixture(context, "repo", branch, "unused", "file", "a".repeat(40), "unused", "fixture"))
        .rejects.toThrow("unchanged owned branch");
    }
    expect(observed).toHaveBeenCalledTimes(1);
    vi.restoreAllMocks();
  });

  it.each(["abandoned", "active", "completed", "wrong-target", "unconfirmed"])(
    "requires owned abandoned PR before deleting paired refs: %s", async (mode) => {
      const sourcesDir = await mkdtemp(join(tmpdir(), "ado-aw-patch-cleanup-"));
      const rest = new AdoRest({ orgUrl: "https://dev.azure.com/org", project: "project", token: "test" });
      const deleted = vi.spyOn(rest, "deleteRef").mockResolvedValue();
      const abandon = vi.spyOn(rest, "abandonPullRequest").mockResolvedValue();
      vi.spyOn(rest, "getPullRequest").mockResolvedValue({
        pullRequestId: 42, title: "owned (do not merge)", status: mode,
        sourceRefName: "refs/heads/source", targetRefName: mode === "wrong-target" ? "refs/heads/other" : "refs/heads/target",
      });
      const context: ScenarioContext = {
        orgUrl: "https://dev.azure.com/org", project: "project", token: "test", rest,
        adoRepo: "repo", buildId: "42", adoAwBin: "unused", workDir: sourcesDir,
        log: () => {}, prefix: () => "owned",
      };
      const scenario = createPullRequestScenarios.find((candidate) => candidate.id === "create-pull-request-native-copy")!;
      try {
        const result = scenario.cleanup(context, {
          sourcesDir, rest, repo: "repo", sourceBranch: "source", ownedTarget: "target",
          prId: mode === "unconfirmed" ? undefined : 42,
        });
        if (mode === "abandoned") {
          await expect(result).resolves.toBeUndefined();
          expect(deleted.mock.calls).toEqual([["repo", "refs/heads/source"], ["repo", "refs/heads/target"]]);
        } else {
          await expect(result).rejects.toThrow();
          expect(deleted).not.toHaveBeenCalled();
        }
        if (mode === "wrong-target" || mode === "unconfirmed") expect(abandon).not.toHaveBeenCalled();
        await expect(stat(sourcesDir)).rejects.toMatchObject({ code: "ENOENT" });
      } finally {
        vi.restoreAllMocks();
        await rm(sourcesDir, { recursive: true, force: true });
      }
    },
  );
});
