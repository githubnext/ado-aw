/**
 * Git safe-output scenarios against the ADO `agent-definitions` repo:
 * create-branch, create-git-tag.
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import type { Scenario, ScenarioContext } from "../scenario.js";
import { detBody } from "./common.js";

async function defaultBranchShortName(ctx: ScenarioContext, repo: string): Promise<string> {
  const info = await ctx.rest.getRepository(repo);
  return (info.defaultBranch ?? "refs/heads/main").replace(/^refs\/heads\//, "");
}

export const createBranch: Scenario<{ repo: string; branch: string; base: string }> = {
  tool: "create-branch",
  targetsAdoRepo: true,
  config: (ctx) => ({ "allowed-repositories": [ctx.adoRepo], max: 1 }),
  setup: async (ctx) => ({
    repo: ctx.adoRepo,
    branch: ctx.prefix("create-branch"),
    base: await defaultBranchShortName(ctx, ctx.adoRepo),
  }),
  ndjson: async (ctx, state) => ({
    branch_name: state.branch,
    source_branch: state.base,
    repository: ctx.adoRepo,
  }),
  assert: async (ctx, state) => {
    const sha = await ctx.rest.getRefObjectId(state.repo, `heads/${state.branch}`);
    if (!sha) throw new Error(`branch '${state.branch}' was not created`);
  },
  cleanup: async (ctx, state) => ctx.rest.deleteRef(state.repo, `refs/heads/${state.branch}`),
};

export const createGitTag: Scenario<{ repo: string; tag: string }> = {
  tool: "create-git-tag",
  targetsAdoRepo: true,
  config: (ctx) => ({ "allowed-repositories": [ctx.adoRepo], max: 1 }),
  setup: async (ctx) => ({ repo: ctx.adoRepo, tag: `ado-aw-det-${ctx.buildId}-tag` }),
  ndjson: async (ctx, state) => ({
    tag_name: state.tag,
    message: detBody(ctx, "create-git-tag"),
    repository: ctx.adoRepo,
  }),
  assert: async (ctx, state) => {
    const sha = await ctx.rest.getRefObjectId(state.repo, `tags/${state.tag}`);
    if (!sha) throw new Error(`tag '${state.tag}' was not created`);
  },
  cleanup: async (ctx, state) => ctx.rest.deleteRef(state.repo, `refs/tags/${state.tag}`),
};

export const gitScenarios: Scenario<any>[] = [createBranch, createGitTag];
