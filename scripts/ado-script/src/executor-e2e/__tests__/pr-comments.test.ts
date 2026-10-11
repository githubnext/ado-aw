import { afterEach, describe, expect, it, vi } from "vitest";
import { AdoRest } from "../ado-rest.js";
import type { ScenarioContext } from "../scenario.js";
import { prOwnedCommentScenarios } from "../scenarios/pr-comments.js";

afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); });

describe("inline readback deadline", () => {
  it.each([false, true])("bounds the authoritative context read (aborted=%s)", async (aborted) => {
    const ctx: ScenarioContext = {
      orgUrl: "https://dev.azure.com/org", project: "P", adoRepo: "repo",
      buildId: "42", token: "test", adoAwBin: "unused", workDir: "unused",
      log: () => {}, prefix: (id) => `owned-${id}`,
      rest: new AdoRest({ orgUrl: "https://dev.azure.com/org", project: "P", token: "test" }),
    };
    vi.spyOn(ctx.rest, "getThread").mockResolvedValue({
      id: 3, comments: [{ id: 1, content: "Comment on the exact reviewed file revision." }],
    });
    const controller = new AbortController();
    const timeout = vi.spyOn(AbortSignal, "timeout").mockReturnValue(controller.signal);
    const read = vi.fn<typeof fetch>(async (_url, init) => {
      expect(init?.signal).toBe(controller.signal);
      if (aborted) {
        return new Promise<Response>((_resolve, reject) => {
          controller.signal.addEventListener("abort", () => reject(new Error("readback deadline")), { once: true });
          controller.abort();
        });
      }
      return Response.json({
        threadContext: { filePath: "/guide.md", rightFileStart: { line: 1 } },
        pullRequestThreadContext: { changeTrackingId: 4 },
      });
    });
    vi.stubGlobal("fetch", read);
    const scenario = prOwnedCommentScenarios.find((item) => item.id === "pr-inline-right")!;
    const assertion = scenario.assert(ctx, { repo: "repo", prId: 42, file: "guide.md" },
      { name: "add_pull_request_comment", status: "succeeded", result: { thread_id: 3 } }, []);
    if (aborted) await expect(assertion).rejects.toThrow("readback deadline");
    else await expect(assertion).resolves.toBeUndefined();
    expect(timeout).toHaveBeenCalledWith(30_000);
    expect(read).toHaveBeenCalledTimes(1);
  });
});
