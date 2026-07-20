import { describe, expect, it, vi } from "vitest";

import type { AdoRest } from "../../executor-e2e/ado-rest.js";
import { runAll, runScenario, scenarioConcurrency } from "../runner.js";
import { SkipError } from "../scenario.js";
import type { TriggerContext, TriggerScenario } from "../scenario.js";

interface FakeBuild {
  result?: string;
  tags?: string[];
  /** Optional override for the finally-block getBuild status probe. */
  finalStatus?: string;
  /** When set, waitForBuild rejects with this message (queue-phase failure). */
  waitError?: string;
}

function makeCtx(build: FakeBuild): { ctx: TriggerContext; rest: Record<string, ReturnType<typeof vi.fn>> } {
  const rest = {
    queueBuild: vi.fn().mockResolvedValue({ id: 42 }),
    waitForBuild: build.waitError
      ? vi.fn().mockRejectedValue(new Error(build.waitError))
      : vi.fn().mockResolvedValue({ status: "completed", result: build.result }),
    getBuildTags: vi.fn().mockResolvedValue(build.tags ?? []),
    getBuild: vi.fn().mockResolvedValue({ id: 42, status: build.finalStatus ?? "completed" }),
    cancelBuild: vi.fn().mockResolvedValue(undefined),
  };
  const ctx: TriggerContext = {
    orgUrl: "https://dev.azure.com/org/",
    project: "proj",
    adoRepo: "repo",
    buildId: "1",
    token: "t",
    victimDefinitionId: 7,
    rest: rest as unknown as AdoRest,
    log: () => {},
    prefix: (id) => `ado-aw-trig-1-${id}`,
  };
  return { ctx, rest };
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
    const { ctx } = makeCtx({ result: "succeeded", tags: ["trig.should-run.true"] });
    const res = await runScenario(
      ctx,
      scenario({ expected: () => ({ result: "succeeded", tags: ["trig.should-run.true"] }) }),
    );
    expect(res.ok).toBe(true);
  });

  it("fails (assert) when a required tag is missing", async () => {
    const { ctx } = makeCtx({ result: "succeeded", tags: [] });
    const res = await runScenario(ctx, scenario({ expected: () => ({ tags: ["pr-gate.skipped"] }) }));
    expect(res.ok).toBe(false);
    expect(res.phase).toBe("assert");
    expect(res.message).toContain("pr-gate.skipped");
  });

  it("fails (assert) on the wrong build result", async () => {
    const { ctx } = makeCtx({ result: "canceled" });
    const res = await runScenario(ctx, scenario({ expected: () => ({ result: "succeeded" }) }));
    expect(res.ok).toBe(false);
    expect(res.phase).toBe("assert");
  });

  it("fails (assert) when a forbidden tag is present", async () => {
    const { ctx } = makeCtx({ result: "succeeded", tags: ["pr-gate.skipped"] });
    const res = await runScenario(
      ctx,
      scenario({ expected: () => ({ absentTags: ["pr-gate.skipped"] }) }),
    );
    expect(res.ok).toBe(false);
    expect(res.phase).toBe("assert");
  });

  it("records a SkipError from setup as skipped and does NOT run cleanup", async () => {
    const { ctx } = makeCtx({ result: "succeeded" });
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
    const { ctx } = makeCtx({ result: "failed" });
    const cleanup = vi.fn().mockResolvedValue(undefined);
    const res = await runScenario(ctx, scenario({ cleanup }));
    expect(res.ok).toBe(false);
    expect(cleanup).toHaveBeenCalledOnce();
  });

  it("honours a custom assert hook", async () => {
    const { ctx } = makeCtx({ result: "succeeded" });
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

  it("cancels an orphaned victim build when the poll times out (queue phase)", async () => {
    // waitForBuild rejects (timeout); the build is still running afterwards.
    const { ctx, rest } = makeCtx({ waitError: "waitForBuild timed out", finalStatus: "inProgress" });
    const res = await runScenario(ctx, scenario({}));
    expect(res.ok).toBe(false);
    expect(res.phase).toBe("queue");
    // The queued build id (42) must be cancelled so it is not orphaned.
    expect(rest.getBuild).toHaveBeenCalledWith(42);
    expect(rest.cancelBuild).toHaveBeenCalledWith(42);
  });

  it("does NOT cancel a build that already completed", async () => {
    const { ctx, rest } = makeCtx({ result: "succeeded", finalStatus: "completed" });
    const res = await runScenario(ctx, scenario({ expected: () => ({ result: "succeeded" }) }));
    expect(res.ok).toBe(true);
    expect(rest.cancelBuild).not.toHaveBeenCalled();
  });
});

describe("scenarioConcurrency", () => {
  it("defaults to four, including for an unexpanded ADO macro", () => {
    expect(scenarioConcurrency({})).toBe(4);
    expect(scenarioConcurrency({ TRIGGER_E2E_CONCURRENCY: "$(TRIGGER_E2E_CONCURRENCY)" })).toBe(4);
  });

  it("accepts an explicit bounded integer", () => {
    expect(scenarioConcurrency({ TRIGGER_E2E_CONCURRENCY: "6" })).toBe(6);
  });

  it.each(["0", "9", "1.5", "many"])("rejects invalid value %s", (value) => {
    expect(() => scenarioConcurrency({ TRIGGER_E2E_CONCURRENCY: value })).toThrow(
      "TRIGGER_E2E_CONCURRENCY",
    );
  });
});

describe("runAll", () => {
  it("runs scenarios concurrently while preserving declaration order", async () => {
    const { ctx } = makeCtx({ result: "succeeded" });
    let active = 0;
    let maxActive = 0;

    const delayed = (id: string, delayMs: number): TriggerScenario<string> =>
      scenario({
        id,
        setup: async () => {
          active += 1;
          maxActive = Math.max(maxActive, active);
          await new Promise((resolve) => setTimeout(resolve, delayMs));
          active -= 1;
          return id;
        },
      });

    const results = await runAll(
      ctx,
      [delayed("first", 30), delayed("second", 5), delayed("third", 5)],
      2,
    );

    expect(maxActive).toBe(2);
    expect(results.map((result) => result.id)).toEqual(["first", "second", "third"]);
    expect(results.every((result) => result.ok)).toBe(true);
  });

  it("never exceeds the requested concurrency", async () => {
    const { ctx } = makeCtx({ result: "succeeded" });
    let active = 0;
    let maxActive = 0;
    const scenarios = Array.from({ length: 7 }, (_, index) =>
      scenario({
        id: `s-${index}`,
        setup: async () => {
          active += 1;
          maxActive = Math.max(maxActive, active);
          await new Promise((resolve) => setTimeout(resolve, 10));
          active -= 1;
          return "state";
        },
      }),
    );

    await runAll(ctx, scenarios, 3);
    expect(maxActive).toBe(3);
  });
});
