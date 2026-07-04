import { tmpdir } from "node:os";

import { describe, expect, it } from "vitest";

import { runScenario } from "../runner.js";
import { SkipError } from "../scenario.js";
import type { Scenario, ScenarioContext } from "../scenario.js";

function fakeCtx(): ScenarioContext {
  return {
    orgUrl: "https://dev.azure.com/org/",
    project: "P",
    adoRepo: "agent-definitions",
    buildId: "1",
    token: "t",
    adoAwBin: "ado-aw",
    workDir: tmpdir(),
    rest: {} as ScenarioContext["rest"],
    log: () => {},
    prefix: (tool) => `ado-aw-det-1-${tool}`,
  };
}

/** A scenario whose setup throws — the runner must never reach execute. */
function guardScenario(setup: () => Promise<never>): Scenario<unknown> {
  let executed = false;
  return {
    tool: "guard",
    config: () => {
      executed = true;
      return {};
    },
    setup,
    ndjson: async () => {
      executed = true;
      return {};
    },
    assert: async () => {
      executed = true;
    },
    cleanup: async () => {
      if (executed) throw new Error("cleanup should not run after setup failure");
    },
  };
}

describe("runScenario precondition handling", () => {
  it("marks SkipError from setup as skipped, not failed", async () => {
    const scenario = guardScenario(async () => {
      throw new SkipError("no wiki");
    });
    const res = await runScenario(fakeCtx(), scenario);
    expect(res.ok).toBe(true);
    expect(res.skipped).toBe(true);
    expect(res.phase).toBe("skipped");
    expect(res.message).toBe("no wiki");
  });

  it("records a setup failure without reaching execute or cleanup", async () => {
    const scenario = guardScenario(async () => {
      throw new Error("boom");
    });
    const res = await runScenario(fakeCtx(), scenario);
    expect(res.ok).toBe(false);
    expect(res.phase).toBe("setup");
    expect(res.message).toBe("boom");
  });
});
