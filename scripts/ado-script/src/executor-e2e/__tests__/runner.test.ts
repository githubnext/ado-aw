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

/**
 * A scenario whose setup throws — the runner must never reach execute or run
 * cleanup. `flags.executed` trips if any post-setup phase runs; `flags.cleaned`
 * trips if cleanup runs. The runner swallows cleanup errors, so we surface the
 * violation via the returned flags rather than a throw.
 */
function guardScenario(
  setup: () => Promise<never>,
  flags: { executed: boolean; cleaned: boolean },
): Scenario<unknown> {
  return {
    tool: "guard",
    config: () => {
      flags.executed = true;
      return {};
    },
    setup,
    ndjson: async () => {
      flags.executed = true;
      return {};
    },
    assert: async () => {
      flags.executed = true;
    },
    cleanup: async () => {
      flags.cleaned = true;
    },
  };
}

describe("runScenario precondition handling", () => {
  it("marks SkipError from setup as skipped, not failed", async () => {
    const flags = { executed: false, cleaned: false };
    const scenario = guardScenario(async () => {
      throw new SkipError("no wiki");
    }, flags);
    const res = await runScenario(fakeCtx(), scenario);
    expect(res.ok).toBe(true);
    expect(res.skipped).toBe(true);
    expect(res.phase).toBe("skipped");
    expect(res.message).toBe("no wiki");
    expect(flags.executed).toBe(false);
    expect(flags.cleaned).toBe(false);
  });

  it("records a setup failure without reaching execute or cleanup", async () => {
    const flags = { executed: false, cleaned: false };
    const scenario = guardScenario(async () => {
      throw new Error("boom");
    }, flags);
    const res = await runScenario(fakeCtx(), scenario);
    expect(res.ok).toBe(false);
    expect(res.phase).toBe("setup");
    expect(res.message).toBe("boom");
    expect(flags.executed).toBe(false);
    expect(flags.cleaned).toBe(false);
  });
});
