import { describe, expect, it, vi } from "vitest";

import type { AdoRest } from "../../executor-e2e/ado-rest.js";
import { runScenario } from "../runner.js";
import { SkipError } from "../scenario.js";
import type { TriggerContext, TriggerScenario } from "../scenario.js";

interface FakeBuild {
  result?: string;
  tags?: string[];
}

function makeCtx(build: FakeBuild): TriggerContext {
  const rest = {
    queueBuild: vi.fn().mockResolvedValue({ id: 42 }),
    waitForBuild: vi.fn().mockResolvedValue({ status: "completed", result: build.result }),
    getBuildTags: vi.fn().mockResolvedValue(build.tags ?? []),
  } as unknown as AdoRest;
  return {
    orgUrl: "https://dev.azure.com/org/",
    project: "proj",
    adoRepo: "repo",
    buildId: "1",
    token: "t",
    victimDefinitionId: 7,
    rest,
    log: () => {},
    prefix: (id) => `ado-aw-trig-1-${id}`,
  };
}

function scenario(overrides: Partial<TriggerScenario<string>>): TriggerScenario<string> {
  return {
    id: "s",
    description: "test scenario",
    setup: async () => "state",
    queue: () => ({ templateParameters: { gateSpec: "x" } }),
    expected: () => ({ result: "succeeded" }),
    cleanup: async () => {},
    ...overrides,
  };
}

describe("runScenario", () => {
  it("passes when result and tags match", async () => {
    const ctx = makeCtx({ result: "succeeded", tags: ["trig.should-run.true"] });
    const res = await runScenario(
      ctx,
      scenario({ expected: () => ({ result: "succeeded", tags: ["trig.should-run.true"] }) }),
    );
    expect(res.ok).toBe(true);
  });

  it("fails (assert) when a required tag is missing", async () => {
    const ctx = makeCtx({ result: "succeeded", tags: [] });
    const res = await runScenario(ctx, scenario({ expected: () => ({ tags: ["pr-gate.skipped"] }) }));
    expect(res.ok).toBe(false);
    expect(res.phase).toBe("assert");
    expect(res.message).toContain("pr-gate.skipped");
  });

  it("fails (assert) on the wrong build result", async () => {
    const ctx = makeCtx({ result: "canceled" });
    const res = await runScenario(ctx, scenario({ expected: () => ({ result: "succeeded" }) }));
    expect(res.ok).toBe(false);
    expect(res.phase).toBe("assert");
  });

  it("fails (assert) when a forbidden tag is present", async () => {
    const ctx = makeCtx({ result: "succeeded", tags: ["pr-gate.skipped"] });
    const res = await runScenario(
      ctx,
      scenario({ expected: () => ({ absentTags: ["pr-gate.skipped"] }) }),
    );
    expect(res.ok).toBe(false);
    expect(res.phase).toBe("assert");
  });

  it("records a SkipError from setup as skipped and does NOT run cleanup", async () => {
    const ctx = makeCtx({ result: "succeeded" });
    const cleanup = vi.fn().mockResolvedValue(undefined);
    const res = await runScenario(
      ctx,
      scenario({
        setup: async () => {
          throw new SkipError("no repo");
        },
        cleanup,
      }),
    );
    expect(res.skipped).toBe(true);
    expect(res.ok).toBe(true);
    expect(cleanup).not.toHaveBeenCalled();
  });

  it("runs cleanup after a successful setup even when assertion fails", async () => {
    const ctx = makeCtx({ result: "failed" });
    const cleanup = vi.fn().mockResolvedValue(undefined);
    const res = await runScenario(ctx, scenario({ cleanup }));
    expect(res.ok).toBe(false);
    expect(cleanup).toHaveBeenCalledOnce();
  });

  it("honours a custom assert hook", async () => {
    const ctx = makeCtx({ result: "succeeded" });
    const res = await runScenario(
      ctx,
      scenario({
        assert: async () => {
          throw new Error("custom boom");
        },
      }),
    );
    expect(res.ok).toBe(false);
    expect(res.message).toContain("custom boom");
  });
});
