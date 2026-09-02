/**
 * Optional cross-organization Azure Repos write scenarios.
 *
 * These require pre-provisioned same-tenant Azure DevOps WIF infrastructure;
 * they skip when any required environment value is absent.
 */
import { AdoRest } from "../ado-rest.js";
import type { Scenario, ScenarioContext, ScenarioSource } from "../scenario.js";
import { SkipError } from "../scenario.js";
import { detBody } from "./common.js";

export interface CrossOrgEnv {
  organization: string;
  orgUrl: string;
  project: string;
  repository: string;
  alias: string;
  endpoint: string;
  token: string;
  rest: AdoRest;
}

function cleanVar(value: string | undefined): string | undefined {
  const trimmed = value?.trim();
  if (!trimmed || /^\$\([^)]+\)$/.test(trimmed)) return undefined;
  return trimmed;
}

export function resolveCrossOrgEnv(ctx: ScenarioContext): CrossOrgEnv {
  const organization = cleanVar(process.env.EXECUTOR_E2E_CROSS_ORG_ORGANIZATION);
  const project = cleanVar(process.env.EXECUTOR_E2E_CROSS_ORG_PROJECT);
  const repository = cleanVar(process.env.EXECUTOR_E2E_CROSS_ORG_REPOSITORY);
  const endpoint = cleanVar(process.env.EXECUTOR_E2E_CROSS_ORG_ENDPOINT);
  const token = cleanVar(process.env.EXECUTOR_E2E_CROSS_ORG_TOKEN);
  const missing = [
    ["EXECUTOR_E2E_CROSS_ORG_ORGANIZATION", organization],
    ["EXECUTOR_E2E_CROSS_ORG_PROJECT", project],
    ["EXECUTOR_E2E_CROSS_ORG_REPOSITORY", repository],
    ["EXECUTOR_E2E_CROSS_ORG_ENDPOINT", endpoint],
    ["EXECUTOR_E2E_CROSS_ORG_TOKEN", token],
  ].filter(([, value]) => !value).map(([name]) => name);
  if (missing.length > 0) {
    throw new SkipError(
      `cross-org repository scenarios require ${missing.join(", ")}`,
    );
  }
  if (!organization || !project || !repository || !endpoint || !token) {
    throw new SkipError(
      "cross-org repository scenario configuration is incomplete",
    );
  }
  const orgUrl = `https://dev.azure.com/${organization}/`;
  return {
    organization,
    orgUrl,
    project,
    repository,
    alias: "cross-org-target",
    endpoint,
    token,
    rest: new AdoRest({
      orgUrl,
      project,
      token,
      authKind: "bearer",
      log: ctx.log,
    }),
  };
}

export function crossOrgSource(env: CrossOrgEnv): ScenarioSource {
  return {
    repositories: [{
      name: `${env.project}/${env.repository}`,
      alias: env.alias,
      organization: env.organization,
      endpoint: env.endpoint,
    }],
    writePermissions: {
      serviceConnection: env.endpoint,
      connectionType: "azureDevOps",
      allow: [{
        organization: env.organization,
        projects: [{
          project: env.project,
          repositories: [env.repository],
        }],
      }],
    },
  };
}

function defaultBranch(defaultBranch: string | undefined): string {
  return defaultBranch?.replace(/^refs\/heads\//, "") || "main";
}

export const createCrossOrgBranch: Scenario<{
  env: CrossOrgEnv;
  branch: string;
  base: string;
}> = {
  id: "create-branch-cross-org",
  tool: "create-branch",
  setup: async (ctx) => {
    const env = resolveCrossOrgEnv(ctx);
    const repository = await env.rest.getRepository(env.repository);
    return {
      env,
      branch: ctx.prefix("create-branch-cross-org"),
      base: defaultBranch(repository.defaultBranch),
    };
  },
  source: async (_ctx, state) => crossOrgSource(state.env),
  config: (_ctx, state) => ({
    "allowed-repositories": [state.env.alias],
    max: 1,
  }),
  env: async (_ctx, state) => ({
    SYSTEM_ACCESSTOKEN: state.env.token,
  }),
  ndjson: async (_ctx, state) => ({
    branch_name: state.branch,
    source_branch: state.base,
    repository: state.env.alias,
  }),
  assert: async (_ctx, state) => {
    const sha = await state.env.rest.getRefObjectId(
      state.env.repository,
      `heads/${state.branch}`,
    );
    if (!sha) throw new Error(`cross-org branch '${state.branch}' was not created`);
  },
  cleanup: async (_ctx, state) =>
    state.env.rest.deleteRef(state.env.repository, `refs/heads/${state.branch}`),
};

export const createCrossOrgGitTag: Scenario<{
  env: CrossOrgEnv;
  tag: string;
}> = {
  id: "create-git-tag-cross-org",
  tool: "create-git-tag",
  setup: async (ctx) => ({
    env: resolveCrossOrgEnv(ctx),
    tag: `ado-aw-det-${ctx.buildId}-cross-org-tag`,
  }),
  source: async (_ctx, state) => crossOrgSource(state.env),
  config: (_ctx, state) => ({
    "allowed-repositories": [state.env.alias],
    max: 1,
  }),
  env: async (_ctx, state) => ({
    SYSTEM_ACCESSTOKEN: state.env.token,
  }),
  ndjson: async (ctx, state) => ({
    tag_name: state.tag,
    message: detBody(ctx, "create-git-tag-cross-org"),
    repository: state.env.alias,
  }),
  assert: async (_ctx, state) => {
    const sha = await state.env.rest.getRefObjectId(
      state.env.repository,
      `tags/${state.tag}`,
    );
    if (!sha) throw new Error(`cross-org tag '${state.tag}' was not created`);
  },
  cleanup: async (_ctx, state) =>
    state.env.rest.deleteRef(state.env.repository, `refs/tags/${state.tag}`),
};

export const crossOrgScenarios: Scenario<unknown>[] = [
  createCrossOrgBranch,
  createCrossOrgGitTag,
];
