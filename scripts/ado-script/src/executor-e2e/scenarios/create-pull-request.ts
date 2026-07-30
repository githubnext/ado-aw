/**
 * Flagship deterministic scenarios: create-pull-request.
 *
 * Stage 3's create-pull-request executor operates on a real git checkout on
 * disk: it reads a staged patch file, verifies its SHA-256, applies it via
 * `git apply --3way` on top of the target branch, and pushes a new source
 * branch + opens the PR via ADO REST using the recorded `base_commit` as the
 * parent.
 *
 * We reproduce both supported checkout layouts deterministically (no LLM):
 *   - a named additional checkout at `<BUILD_SOURCESDIRECTORY>/<alias>`,
 *   - `repository: self` beneath a non-Git multi-checkout root, located via
 *     `ADO_AW_SELF_REPOSITORY_DIRECTORY`.
 *
 * Each scenario:
 *   - clone the configured ADO test repo into the source-checkout dir,
 *   - record `base_commit` = main HEAD,
 *   - make a deterministic edit and capture a `git diff` patch,
 *   - compute `patch_sha256`,
 *   - run the executor, assert the PR + pushed branch, then abandon + delete.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdir, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";

import type { Scenario, ScenarioContext } from "../scenario.js";
import { partialOutput } from "../execute-cli.js";
import { detBody, numResult, Teardown } from "./common.js";

interface CreatePrState {
  repo: string;
  sourceBranch: string;
  targetBranch: string;
  baseCommit: string;
  patchRelPath: string;
  patchSha256: string;
  patchContent: string;
  /** BUILD_SOURCESDIRECTORY passed to the executor. */
  sourcesDir: string;
  /** Actual git checkout beneath sourcesDir. */
  checkoutDir: string;
  /** ADO repository id, required when the executor selector is `self`. */
  repositoryId?: string;
  /** PR id, populated in assert() so cleanup can abandon it. */
  prId?: number;
}

interface CreatePrScenarioOptions {
  readonly id: string;
  readonly repositorySelector: "named" | "self";
  readonly patchRelPath: string;
  readonly changedFileSuffix?: string;
}

function runGit(
  args: string[],
  cwd: string,
  extraHeader: string,
): Promise<{ code: number; stdout: string; stderr: string }> {
  const timeoutMs = Number(process.env.EXECUTOR_E2E_GIT_TIMEOUT_MS) || 300_000;
  return new Promise((resolve, reject) => {
    // Inject the auth header via GIT_CONFIG_* env vars rather than `-c` on the
    // command line, so the token never appears in the process argv
    // (/proc/<pid>/cmdline). GIT_TERMINAL_PROMPT=0 makes auth failures fail
    // fast instead of blocking on an interactive credential prompt.
    //
    // Append at the next free index rather than clobbering GIT_CONFIG_COUNT to
    // "1", so any GIT_CONFIG_KEY/VALUE_N entries the agent or a variable group
    // already injected are preserved.
    const existingCount = Number.parseInt(process.env.GIT_CONFIG_COUNT ?? "0", 10);
    const idx = Number.isFinite(existingCount) && existingCount > 0 ? existingCount : 0;
    const child = spawn("git", args, {
      cwd,
      env: {
        ...process.env,
        GIT_TERMINAL_PROMPT: "0",
        GIT_CONFIG_COUNT: String(idx + 1),
        [`GIT_CONFIG_KEY_${idx}`]: "http.extraheader",
        [`GIT_CONFIG_VALUE_${idx}`]: `Authorization: ${extraHeader}`,
      },
    });
    let stdout = "";
    let stderr = "";
    let timedOut = false;
    const timer = setTimeout(() => {
      timedOut = true;
      child.kill("SIGKILL");
    }, timeoutMs);
    child.stdout.on("data", (d: Buffer) => (stdout += d.toString()));
    child.stderr.on("data", (d: Buffer) => (stderr += d.toString()));
    child.on("error", (err) => {
      clearTimeout(timer);
      reject(err);
    });
    child.on("close", (code) => {
      clearTimeout(timer);
      if (timedOut) {
        reject(new Error(`git ${args.join(" ")} timed out after ${timeoutMs}ms${partialOutput(stdout, stderr)}`));
        return;
      }
      resolve({ code: code ?? -1, stdout, stderr });
    });
  });
}

async function git(
  ctx: ScenarioContext,
  args: string[],
  cwd: string,
  extraHeader: string,
  scenarioId: string,
): Promise<string> {
  // Safe to log: the auth token is passed via env (GIT_CONFIG_*), not argv.
  ctx.log(`[${scenarioId}] git ${args.join(" ")}`);
  const res = await runGit(args, cwd, extraHeader);
  if (res.code !== 0) {
    throw new Error(`git ${args.join(" ")} failed (${res.code}): ${res.stderr.trim()}`);
  }
  return res.stdout;
}

async function setupCreatePullRequest(
  ctx: ScenarioContext,
  options: CreatePrScenarioOptions,
): Promise<CreatePrState> {
  const repo = ctx.adoRepo;
  const authHeader = "Basic " + Buffer.from(":" + ctx.token).toString("base64");
  const sourcesDir = join(ctx.workDir, options.id, "src-checkout");
  await mkdir(sourcesDir, { recursive: true });
  const checkoutDir = join(sourcesDir, repo);
  const repositoryId =
    options.repositorySelector === "self"
      ? (await ctx.rest.getRepository(repo)).id
      : undefined;

  const cloneUrl = `${ctx.orgUrl.replace(/\/+$/, "")}/${encodeURIComponent(ctx.project)}/_git/${encodeURIComponent(repo)}`;
  ctx.log(`[${options.id}] cloning ${repo}`);
  await git(ctx, ["clone", cloneUrl, checkoutDir], sourcesDir, authHeader, options.id);

  // Determine the default branch and its HEAD (the patch base commit).
  // `symbolic-ref refs/remotes/origin/HEAD` exits non-zero (not empty) when
  // the remote HEAD symref isn't configured, which git() turns into a throw.
  // Catch that and fall back to "main" so the `|| "main"` isn't dead code.
  let targetBranch = "main";
  try {
    const symref = await git(
      ctx,
      ["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
      checkoutDir,
      authHeader,
      options.id,
    );
    targetBranch = symref.trim().replace(/^origin\//, "") || "main";
  } catch {
    ctx.log(`[${options.id}] origin/HEAD symref not set; defaulting target branch to 'main'`);
  }
  const baseCommit = (
    await git(ctx, ["rev-parse", "HEAD"], checkoutDir, authHeader, options.id)
  ).trim();

  // Deterministic edit: add a new file, then capture a git diff (which
  // `git apply --3way` applies cleanly on top of the target branch).
  // The buildId-scoped path is assumed untracked: `git add -N` on an
  // already-tracked path is a no-op, so `git diff` would be empty and setup
  // would (cleanly) fail with "generated patch is empty".
  const relFile = `ado-aw-det/${ctx.buildId}${options.changedFileSuffix ?? ""}.md`;
  const absFile = join(checkoutDir, relFile);
  await mkdir(join(absFile, ".."), { recursive: true });
  await writeFile(absFile, `${detBody(ctx, options.id)}\n`, "utf8");
  await git(ctx, ["add", "-N", relFile], checkoutDir, authHeader, options.id);
  const patchContent = await git(
    ctx,
    ["diff", "--", relFile],
    checkoutDir,
    authHeader,
    options.id,
  );
  if (!patchContent.trim()) throw new Error("generated patch is empty");
  // Reset the intent-to-add so the checkout stays clean.
  await git(ctx, ["reset", "--", relFile], checkoutDir, authHeader, options.id);

  const patchSha256 = createHash("sha256").update(patchContent, "utf8").digest("hex");

  return {
    repo,
    sourceBranch: ctx.prefix(options.id),
    targetBranch,
    baseCommit,
    patchRelPath: options.patchRelPath,
    patchSha256,
    patchContent,
    sourcesDir,
    checkoutDir,
    repositoryId,
  };
}

function createPullRequestScenario(
  options: CreatePrScenarioOptions,
): Scenario<CreatePrState> {
  return {
    id: options.id,
    tool: "create-pull-request",
    targetsAdoRepo: true,
    config: (_ctx, state) => ({
      // Target the repo's actual default branch (state.targetBranch), which is
      // also where base_commit was taken from, rather than hardcoding "main".
      "target-branch": state.targetBranch,
      "allowed-repositories": [state.repo],
      "delete-source-branch": true,
      "if-no-changes": "error",
      "include-stats": false,
    }),
    setup: (ctx) => setupCreatePullRequest(ctx, options),
    files: async (_ctx, state) => ({ [state.patchRelPath]: state.patchContent }),
    env: async (_ctx, state) => {
      const env: Record<string, string> = {
        BUILD_SOURCESDIRECTORY: state.sourcesDir,
      };
      if (options.repositorySelector === "self") {
        if (!state.repositoryId) {
          throw new Error("self create-pull-request scenario has no ADO repository id");
        }
        Object.assign(env, {
          ADO_AW_SELF_REPOSITORY_DIRECTORY: state.checkoutDir,
          ADO_AW_SELF_REPOSITORY_ID: state.repositoryId,
          ADO_AW_SELF_REPOSITORY_NAME: state.repo,
          // Model a repository-resource-triggered run: Build.Repository.*
          // identifies the trigger, while the compiler-owned values above
          // must continue to identify checkout: self.
          BUILD_REPOSITORY_ID: "00000000-0000-0000-0000-000000000000",
          BUILD_REPOSITORY_NAME: "external-trigger-repository",
        });
      }
      return env;
    },
    ndjson: async (ctx, state) => ({
      title: `${ctx.prefix(options.id)} (do not merge)`,
      description: detBody(ctx, options.id),
      source_branch: state.sourceBranch,
      patch_file: state.patchRelPath,
      repository: options.repositorySelector === "self" ? "self" : state.repo,
      agent_labels: [],
      base_commit: state.baseCommit,
      patch_sha256: state.patchSha256,
    }),
    assert: async (ctx, state, record) => {
      const prId = numResult(record, "pull_request_id");
      // Record the PR id up front so cleanup abandons it even if a later
      // assertion (or the getPullRequest call itself) throws.
      state.prId = prId;
      const pr = await ctx.rest.getPullRequest(state.repo, prId);
      if (pr.status === "abandoned") throw new Error(`PR #${prId} is abandoned`);
      const sha = await ctx.rest.getRefObjectId(state.repo, `heads/${state.sourceBranch}`);
      if (!sha) throw new Error(`source branch '${state.sourceBranch}' was not pushed`);
    },
    cleanup: async (ctx, state) => {
      // Attempt each cleanup independently so an early failure never skips the
      // rest — otherwise a throwing abandonPullRequest would leak the source
      // branch and the local checkout dir.
      const teardown = new Teardown();
      if (state.prId !== undefined) {
        const prId = state.prId;
        teardown.add("abandon PR", () => ctx.rest.abandonPullRequest(state.repo, prId));
      }
      await teardown
        .add("delete source branch", () =>
          ctx.rest.deleteRef(state.repo, `refs/heads/${state.sourceBranch}`),
        )
        // Remove the cloned checkout so repeated local runs don't accumulate it.
        .add("remove local checkout", () =>
          rm(state.sourcesDir, { recursive: true, force: true }),
        )
        .run();
    },
  };
}

export const createPullRequest = createPullRequestScenario({
  id: "create-pull-request",
  repositorySelector: "named",
  patchRelPath: "create-pr.patch",
});

export const createPullRequestSelfMultiCheckout = createPullRequestScenario({
  id: "create-pull-request-self-multi-checkout",
  repositorySelector: "self",
  patchRelPath: "create-pr-self-multi-checkout.patch",
  changedFileSuffix: "-self-multi-checkout",
});

export const createPullRequestScenarios: Scenario<unknown>[] = [
  createPullRequest,
  createPullRequestSelfMultiCheckout,
];
