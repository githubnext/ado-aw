import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { runBypass } from "./bypass.js";
import { _resetCompletedForTesting } from "../shared/vso-logger.js";
import type { GateSpec } from "../shared/types.gen.js";

const baseSpec: GateSpec = {
  context: {
    build_reason: "PullRequest",
    tag_prefix: "pr-gate",
    step_name: "prGate",
    bypass_label: "Pull Request",
  },
  facts: [],
  checks: [],
};

describe("runBypass", () => {
  let writes: string[];
  beforeEach(() => {
    writes = [];
    _resetCompletedForTesting();
    vi.spyOn(process.stdout, "write").mockImplementation((c: any) => {
      writes.push(typeof c === "string" ? c : c.toString());
      return true;
    });
  });
  afterEach(() => vi.restoreAllMocks());

  it("returns true and emits SHOULD_RUN=true when build reason mismatches", async () => {
    process.env.ADO_BUILD_REASON = "Manual";
    const result = await runBypass(baseSpec);
    expect(result).toBe(true);
    const joined = writes.join("");
    expect(joined).toContain("Not a Pull Request build");
    expect(joined).toContain("setvariable variable=SHOULD_RUN;isOutput=true]true");
    expect(joined).toContain("##vso[build.addbuildtag]pr-gate.passed");
    expect(joined).toContain("##vso[task.complete result=Succeeded;]");
  });

  it("emits a build tag that contains no ':' (ADO rejects ':' in the tag REST path)", async () => {
    process.env.ADO_BUILD_REASON = "Manual";
    await runBypass(baseSpec);
    const tagLine = writes.find((w) => w.startsWith("##vso[build.addbuildtag]"));
    expect(tagLine).toBeDefined();
    expect((tagLine as string).slice("##vso[build.addbuildtag]".length)).not.toContain(":");
  });

  it("returns false when build reason matches (no bypass)", async () => {
    process.env.ADO_BUILD_REASON = "PullRequest";
    const result = await runBypass(baseSpec);
    expect(result).toBe(false);
    expect(writes.join("")).not.toContain("setvariable");
  });

  it("returns true when ADO_BUILD_REASON is missing (treated as empty string, mismatches)", async () => {
    delete process.env.ADO_BUILD_REASON;
    const result = await runBypass(baseSpec);
    expect(result).toBe(true);
  });

  it("escapes an adversarial bypass_label so it cannot smuggle vso commands", async () => {
    process.env.ADO_BUILD_REASON = "Manual";
    const adversarial: GateSpec = {
      ...baseSpec,
      context: {
        ...baseSpec.context,
        bypass_label: "##vso[task.complete result=Failed;]X\nY",
      },
    };
    const result = await runBypass(adversarial);
    expect(result).toBe(true);
    // The embedded newline must be encoded so it can't start a fresh
    // ADO-interpreted line. The `##vso[` *inside* the label is allowed
    // because it isn't at line-start (preceded by "Not a "), but we
    // assert structurally that no second `##vso[task.complete result=Failed`
    // command was emitted by the label itself — only the legitimate
    // Succeeded complete from the bypass path.
    const failedCompletes = writes.filter((w) =>
      w.startsWith("##vso[task.complete result=Failed"),
    );
    expect(failedCompletes).toEqual([]);
    expect(writes.join("")).toContain("%0A"); // embedded \n encoded
  });
});
