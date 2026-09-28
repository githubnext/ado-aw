import { createHash } from "node:crypto";
import { mkdir, rm, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import type { Scenario, ScenarioContext } from "../scenario.js";
import { setupPr, teardownPr, type PrState } from "./pr.js";
import { git } from "./create-pull-request.js";

interface State extends PrState {
  head: string;
  sources: string;
  checkout: string;
  patch: string;
  path: string;
  original: string;
}

type Mode = "success" | "stale" | "blocked-branch" | "protected" | "bad-hash" | "empty";

async function setup(ctx: ScenarioContext, mode: Mode): Promise<State> {
  const id = `pr-push-${mode}`;
  const pr = await setupPr(ctx, id, false, true);
  const sources = join(ctx.workDir, id, "source-checkouts");
  const checkout = join(sources, pr.repo);
  try {
    await mkdir(sources, { recursive: true });
    const header = "Basic " + Buffer.from(`:${ctx.token}`).toString("base64");
    const remote = `${ctx.orgUrl.replace(/\/+$/, "")}/${encodeURIComponent(ctx.project)}/_git/${encodeURIComponent(pr.repo)}`;
    await git(ctx, ["clone", "--depth=1", "--branch", pr.branch, remote, checkout], sources, header, id);
    const head = (await git(ctx, ["rev-parse", "HEAD"], checkout, header, id)).trim();
    const path = mode === "protected" ? "package.json" : `ado-aw-det/${ctx.buildId}/${id}-applied.txt`;
    const original = `ado-aw-det/${ctx.buildId}/${id}.md`;
    if (mode !== "empty") {
      await mkdir(dirname(join(checkout, path)), { recursive: true });
      await writeFile(join(checkout, path), `Applied PR source delta for ${ctx.buildId}.\n`, "utf8");
      await git(ctx, ["add", "--", path], checkout, header, id);
    }
    const patch = await git(ctx, ["diff", "--cached", "--binary", "--full-index", "--no-renames", head, "--"],
      checkout, header, id);
    return { ...pr, head, sources, checkout, patch, path, original };
  } catch (error) {
    await teardownPr(ctx, pr);
    await rm(sources, { recursive: true, force: true });
    throw error;
  }
}

const cases = (["success", "stale", "blocked-branch", "protected", "bad-hash", "empty"] as const)
  .map((mode): Scenario<State> => ({
    id: `pr-push-${mode}`,
    tool: "push-to-pull-request-branch",
    targetsAdoRepo: true,
    config: (ctx) => ({
      target: "*", "allowed-repositories": [ctx.adoRepo],
      "allowed-branches": [mode === "blocked-branch" ? "not-allowed/*" : `${ctx.prefix("")}*`],
      "if-no-changes": "ignore",
    }),
    setup: (ctx) => setup(ctx, mode),
    env: async (_ctx, state) => ({
      BUILD_SOURCESDIRECTORY: state.sources,
      ADO_AW_SELF_REPOSITORY_DIRECTORY: state.checkout,
    }),
    files: async (_ctx, state) => ({ "push.patch": state.patch }),
    ndjson: async (_ctx, state) => ({
      pull_request_id: state.prId, repository: state.repo,
      expected_head_sha: mode === "stale" ? "f".repeat(40) : state.head,
      patch_file: "push.patch",
      patch_sha256: mode === "bad-hash" ? "0".repeat(64) : createHash("sha256").update(state.patch).digest("hex"),
    }),
    ...(mode !== "success" && mode !== "empty" ? {
      expectedFailure: { error: {
        stale: /source head changed/,
        "blocked-branch": /allowed-branches/,
        protected: /protected files/,
        "bad-hash": /SHA-256 mismatch/,
      }[mode] },
    } : {}),
    assertFailure: async (ctx, state) => {
      if (await ctx.rest.getRefObjectId(state.repo, `heads/${state.branch}`) !== state.head) {
        throw new Error("Rejected push changed the remote source head");
      }
    },
    assert: async (ctx, state, record) => {
      const head = await ctx.rest.getRefObjectId(state.repo, `heads/${state.branch}`);
      if (mode === "empty") {
        if (head !== state.head) throw new Error("Empty push changed the source branch");
        return;
      }
      if (head === state.head || record.result?.push_status !== "confirmed" || record.result?.commit_id !== head) {
        throw new Error("Push did not confirm the exact new source head");
      }
      const header = "Basic " + Buffer.from(`:${ctx.token}`).toString("base64");
      await git(ctx, ["fetch", "--depth=2", "origin", state.branch], state.checkout, header, `pr-push-${mode}`);
      const applied = await git(ctx, ["show", `FETCH_HEAD:${state.path}`], state.checkout, header, `pr-push-${mode}`);
      if (applied !== `Applied PR source delta for ${ctx.buildId}.\n`) throw new Error("Applied file content differs");
      const original = await git(ctx, ["show", `FETCH_HEAD:${state.original}`], state.checkout, header, `pr-push-${mode}`);
      if (!original.includes(`build ${ctx.buildId}`)) throw new Error("Push lost the PR's pre-existing change");
      const parent = (await git(ctx, ["rev-parse", "FETCH_HEAD^"], state.checkout, header, `pr-push-${mode}`)).trim();
      if (parent !== state.head) throw new Error("Push was not a direct child of the expected PR source head");
    },
    cleanup: async (ctx, state) => {
      try { await teardownPr(ctx, state); }
      finally { await rm(state.sources, { recursive: true, force: true }); }
    },
  }));

export const prPushScenarios: Scenario<unknown>[] = cases as Scenario<unknown>[];
