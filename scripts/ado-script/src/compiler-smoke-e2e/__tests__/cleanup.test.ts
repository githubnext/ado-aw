import { describe, expect, it, vi } from "vitest";
import { cleanupCaseResources } from "../cleanup.js";
import type { OwnedBoundaryPr } from "../ado-rest.js";

const source = { ref: "refs/heads/ado-aw-smoke-candidate/42/check", sha: "a".repeat(40) };
const target = { ref: "refs/heads/ado-aw-smoke-boundary-target/42/check", sha: "b".repeat(40) };
const pr: OwnedBoundaryPr = {
  pullRequestId: 7, status: "active", title: "ado-aw-boundary-original-42-check",
  sourceRefName: source.ref, targetRefName: target.ref,
};

function setup(found: OwnedBoundaryPr | undefined = pr) {
  const order: string[] = [];
  const client = {
    findBoundaryPr: vi.fn(async (): Promise<OwnedBoundaryPr | undefined> => found),
    abandonBoundaryPr: vi.fn(async () => { order.push("abandon"); }),
  };
  const deleteRefs = vi.fn(async () => { order.push("delete"); });
  const opts = { client, repository: "mirror", sourceRef: source.ref, refs: [source, target], deleteRefs };
  return { client, deleteRefs, opts, order };
}

describe("paired case cleanup", () => {
  it("confirms PR abandonment before either leased ref is deleted", async () => {
    const test = setup();
    await cleanupCaseResources({ ...test.opts, expectedPrId: 7 });
    expect(test.order).toEqual(["abandon", "delete"]);
    expect(test.deleteRefs).toHaveBeenCalledWith([source, target]);
  });

  it.each(["discovery", "abandonment", "wrong-id"])("retains both refs after %s failure", async (failure) => {
    const test = setup();
    if (failure === "discovery") test.client.findBoundaryPr.mockRejectedValue(new Error("lookup failed"));
    if (failure === "abandonment") test.client.abandonBoundaryPr.mockRejectedValue(new Error("unconfirmed"));
    await expect(cleanupCaseResources({
      ...test.opts, expectedPrId: failure === "wrong-id" ? 8 : 7,
    })).rejects.toThrow();
    expect(test.deleteRefs).not.toHaveBeenCalled();
  });

  it("recovers a PR whose setup response was lost", async () => {
    const test = setup();
    await cleanupCaseResources(test.opts);
    expect(test.client.abandonBoundaryPr).toHaveBeenCalledWith("mirror", pr);
    expect(test.order).toEqual(["abandon", "delete"]);
  });

  it("cleans a confirmed target-only orphan in the new namespace", async () => {
    const test = setup();
    test.client.findBoundaryPr.mockResolvedValue(undefined);
    await cleanupCaseResources({ ...test.opts, refs: [target] });
    expect(test.client.abandonBoundaryPr).not.toHaveBeenCalled();
    expect(test.deleteRefs).toHaveBeenCalledWith([target]);
  });

  it("requires corroboration for a legacy orphan", async () => {
    const test = setup();
    test.client.findBoundaryPr.mockResolvedValue(undefined);
    await expect(cleanupCaseResources({
      ...test.opts, refs: [{ ...target, ref: `${source.ref}-target` }],
    })).rejects.toThrow("ownership");
    expect(test.deleteRefs).not.toHaveBeenCalled();
  });

  it("recovers a legacy pair only through its validated PR identity", async () => {
    const legacy = { ...target, ref: `${source.ref}-target` };
    const test = setup({ ...pr, targetRefName: legacy.ref });
    await cleanupCaseResources({ ...test.opts, refs: [source, legacy] });
    expect(test.order).toEqual(["abandon", "delete"]);
  });

  it("does not delete the source when its observed target was withheld", async () => {
    const test = setup();
    await expect(cleanupCaseResources({
      ...test.opts, refs: [source], observedRefs: [source, target],
    })).rejects.toThrow("unproven or ambiguous");
    expect(test.client.abandonBoundaryPr).not.toHaveBeenCalled();
    expect(test.deleteRefs).not.toHaveBeenCalled();
  });
});
