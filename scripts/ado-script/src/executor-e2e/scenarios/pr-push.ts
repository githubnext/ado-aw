import { createHash } from "node:crypto";
import { copyFile, mkdir, readFile, rename, rm, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import type { Scenario, ScenarioContext } from "../scenario.js";
import { setupPr, teardownPr, type PrState } from "./pr.js";
import { git, seedExecutableFixture } from "./create-pull-request.js";
import { Teardown } from "./common.js";

interface State extends PrState {
  head: string;
  sources: string;
  checkout: string;
  patch: string;
  path: string;
  original: string;
  expected?: string;
  expectedBlob?: string;
  expectedMode?: string;
  omittedCopy?: string;
}

type Mode = "success" | "stale" | "blocked-branch" | "protected" | "bad-hash" | "empty"
  | "native-copy" | "native-rename" | "excluded-native-copy" | "crlf" | "binary" | "expansion-denied"
  | "space-edit" | "space-rename-edit" | "mode-up-denied" | "mode-down-denied" | "mode-new-denied" | "mode-rename-denied" | "mode-edit";

const failures: Partial<Record<Mode, RegExp>> = {
  stale: /source head changed/, "blocked-branch": /allowed-branches/,
  protected: /protected files/, "bad-hash": /SHA-256 mismatch/,
  "expansion-denied": /pre-application expansion/,
  "mode-up-denied": /file-mode change/, "mode-down-denied": /file-mode change/,
  "mode-new-denied": /file-mode change/, "mode-rename-denied": /file-mode change/,
};

async function setup(ctx: ScenarioContext, mode: Mode): Promise<State> {
  const id = `pr-push-${mode}`;
  const original = `ado-aw-det/${ctx.buildId}/${mode.startsWith("space-") ? "space dir/" : ""}${id}.md`;
  const pr = await setupPr(ctx, id, false, true,
    mode === "expansion-denied" ? "x".repeat(429_575) :
      mode.startsWith("space-") ? `Original PR fixture for build ${ctx.buildId}.\n`.repeat(20) : undefined,
    `/${original}`);
  const sources = join(ctx.workDir, id, "source-checkouts");
  const checkout = join(sources, pr.repo);
  try {
    await mkdir(sources, { recursive: true });
    const header = "Basic " + Buffer.from(`:${ctx.token}`).toString("base64");
    const remote = `${ctx.orgUrl.replace(/\/+$/, "")}/${encodeURIComponent(ctx.project)}/_git/${encodeURIComponent(pr.repo)}`;
    await git(ctx, ["clone", "--depth=1", "--branch", pr.branch, remote, checkout], sources, header, id);
    let head = (await git(ctx, ["rev-parse", "HEAD"], checkout, header, id)).trim();
    if (["mode-down-denied", "mode-rename-denied", "mode-edit"].includes(mode)) {
      head = await seedExecutableFixture(ctx, pr.repo, pr.branch, checkout, original, head, header, id);
    }
    const path = mode === "protected" ? "package.json" :
      ["space-edit", "mode-up-denied", "mode-down-denied", "mode-edit"].includes(mode) ? original :
        `ado-aw-det/${ctx.buildId}/${id}${mode === "space-rename-edit" ? " renamed guide" : "-applied"}.txt`;
    let expected: string | undefined = `Applied PR source delta for ${ctx.buildId}.\n`;
    let expectedBlob: string | undefined;
    let expectedMode: string | undefined;
    let omittedCopy: string | undefined;
    if (mode === "crlf") await git(ctx, ["config", "core.autocrlf", "true"], checkout, header, id);
    if (["native-copy", "native-rename", "space-rename-edit", "mode-rename-denied"].includes(mode)) {
      expected = await readFile(join(checkout, original), "utf8");
      if (mode === "native-copy") await copyFile(join(checkout, original), join(checkout, path));
      else await rename(join(checkout, original), join(checkout, path));
      if (mode === "space-rename-edit") {
        expected += "Additional content.\n";
        await writeFile(join(checkout, path), expected, "utf8");
      }
    } else if (!["empty", "expansion-denied", "mode-up-denied", "mode-down-denied"].includes(mode)) {
      await mkdir(dirname(join(checkout, path)), { recursive: true });
      if (mode === "binary") {
        await writeFile(join(checkout, path), Buffer.from([0, 255, 128, 10]));
        expected = undefined;
      } else {
        await writeFile(join(checkout, path), expected, "utf8");
      }
    }
    if (mode === "excluded-native-copy") {
      omittedCopy = `${path}.excluded`;
      await copyFile(join(checkout, original), join(checkout, omittedCopy));
    }
    let patch: string;
    if (mode === "expansion-denied") {
      patch = Array.from({ length: 99 }, (_, index) =>
        `diff --git a/${original} b/expanded-${index}.txt\nsimilarity index 100%\ncopy from ${original}\ncopy to expanded-${index}.txt\n`).join("");
    } else {
      await git(ctx, ["add", "-A"], checkout, header, id);
      if (mode.startsWith("mode-") && mode !== "mode-edit") {
        await git(ctx, ["update-index", mode === "mode-down-denied" ? "--chmod=-x" : "--chmod=+x", "--", path], checkout, header, id);
      }
      if (mode !== "empty") expectedBlob = (await git(ctx, ["rev-parse", `:${path}`], checkout, header, id)).trim();
      if (mode !== "empty") expectedMode = (await git(ctx, ["--literal-pathspecs", "ls-files", "--stage", "--", path], checkout, header, id)).split(" ")[0];
      patch = await git(ctx, ["diff", "--cached", "--binary", "--full-index",
        "--find-renames", "--find-copies", "--find-copies-harder", head, "--"], checkout, header, id);
    }
    if ((mode === "native-copy" || mode === "excluded-native-copy") && !patch.includes("copy from ")) {
      throw new Error("Native copy fixture did not contain native copy metadata");
    }
    if (["native-rename", "space-rename-edit"].includes(mode) && !patch.includes("rename from ")) {
      throw new Error("Native rename fixture did not contain native rename metadata");
    }
    return { ...pr, head, sources, checkout, patch, path, original, expected, expectedBlob, expectedMode, omittedCopy };
  } catch (error) {
    try {
      await new Teardown().add("clean PR", () => teardownPr(ctx, pr))
        .add("remove checkout", () => rm(sources, { recursive: true, force: true })).run();
    } catch (cleanup) {
      throw new AggregateError([error, cleanup], "PR patch setup and cleanup failed");
    }
    throw error;
  }
}

const cases = (["success", "stale", "blocked-branch", "protected", "bad-hash", "empty",
  "native-copy", "native-rename", "excluded-native-copy", "crlf", "binary", "expansion-denied",
  "space-edit", "space-rename-edit", "mode-up-denied", "mode-down-denied", "mode-new-denied", "mode-rename-denied", "mode-edit"] as const)
  .map((mode): Scenario<State> => ({
    id: `pr-push-${mode}`,
    tool: "push-to-pull-request-branch",
    targetsAdoRepo: true,
    config: (ctx) => ({
      target: "*", "allowed-repositories": [ctx.adoRepo],
      "allowed-branches": [mode === "blocked-branch" ? "not-allowed/*" : `${ctx.prefix("")}*`],
      "if-no-changes": "ignore",
      ...(mode === "excluded-native-copy" ? { "excluded-files": ["pr-push-excluded-native-copy.md"] } : {}),
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
    ...(failures[mode] ? {
      expectedFailure: { error: failures[mode] },
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
      const blob = (await git(ctx, ["rev-parse", `FETCH_HEAD:${state.path}`], state.checkout, header, `pr-push-${mode}`)).trim();
      if (blob !== state.expectedBlob) throw new Error("Applied Git blob differs from the proposed exact bytes");
      const treeMode = (await git(ctx, ["--literal-pathspecs", "ls-tree", "FETCH_HEAD", "--", state.path],
        state.checkout, header, `pr-push-${mode}`)).split(" ")[0];
      if (treeMode !== state.expectedMode) throw new Error("Applied tree-entry mode differs");
      if (state.expected !== undefined) {
        const applied = await git(ctx, ["show", `FETCH_HEAD:${state.path}`], state.checkout, header, `pr-push-${mode}`);
        if (applied !== state.expected) throw new Error("Applied file content differs");
      }
      const files = (await git(ctx, ["ls-tree", "-r", "--name-only", "FETCH_HEAD"], state.checkout, header, `pr-push-${mode}`))
        .trim().split("\n");
      if (["native-rename", "space-rename-edit"].includes(mode)) {
        if (files.includes(state.original)) throw new Error("Native rename retained the source path");
      } else if (state.path !== state.original) {
        const original = await git(ctx, ["show", `FETCH_HEAD:${state.original}`], state.checkout, header, `pr-push-${mode}`);
        if (!original.includes(`build ${ctx.buildId}`)) throw new Error("Push lost the PR's pre-existing change");
      }
      if (state.omittedCopy && (files.includes(state.omittedCopy) || !Array.isArray(record.result?.omitted_operations)
        || record.result.omitted_operations.length !== 1)) {
        throw new Error("Excluded native copy was not omitted and reported as one operation");
      }
      const parent = (await git(ctx, ["rev-parse", "FETCH_HEAD^"], state.checkout, header, `pr-push-${mode}`)).trim();
      if (parent !== state.head) throw new Error("Push was not a direct child of the expected PR source head");
    },
    cleanup: async (ctx, state) => {
      try { await teardownPr(ctx, state); }
      finally { await rm(state.sources, { recursive: true, force: true }); }
    },
  }));

export const prPushScenarios: Scenario<unknown>[] = cases as Scenario<unknown>[];
