import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { runBypass } from "./bypass.js";
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
    expect(joined).toContain("##vso[build.addbuildtag]pr-gate:passed");
    expect(joined).toContain("##vso[task.complete result=Succeeded;]");
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
});
