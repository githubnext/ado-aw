import type { Scenario, ScenarioContext } from "../scenario.js";
import { crossOrgSource, resolveCrossOrgEnv, type CrossOrgEnv } from "./cross-org.js";
import {
  updatePullRequest, addPrLabels, addPrReviewers, submitPrReview,
  setPrAutoComplete, abandonPullRequest,
} from "./pr.js";

function crossOrgPr<S>(scenario: Scenario<S>): Scenario<{ env: CrossOrgEnv; state: S }> {
  const targetContext = (ctx: ScenarioContext, env: CrossOrgEnv): ScenarioContext => ({
    ...ctx, orgUrl: env.orgUrl, project: env.project, adoRepo: env.repository,
    token: env.token, rest: env.rest,
    prefix: (tool) => ctx.prefix(`${tool}-cross-org`),
  });
  return {
    id: `${scenario.id ?? scenario.tool}-cross-org`,
    tool: scenario.tool,
    setup: async (ctx) => {
      const env = resolveCrossOrgEnv(ctx);
      return { env, state: await scenario.setup(targetContext(ctx, env)) };
    },
    source: async (_ctx, state) => crossOrgSource(state.env),
    config: (ctx, state) => ({
      ...scenario.config(targetContext(ctx, state.env), state.state),
      "allowed-repositories": [state.env.alias],
    }),
    env: async (_ctx, state) => ({ SYSTEM_ACCESSTOKEN: state.env.token }),
    ndjson: async (ctx, state) => ({
      ...await scenario.ndjson(targetContext(ctx, state.env), state.state),
      repository: state.env.alias,
    }),
    assert: async (ctx, state, record, records) =>
      scenario.assert(targetContext(ctx, state.env), state.state, record, records),
    cleanup: async (ctx, state, records) =>
      scenario.cleanup(targetContext(ctx, state.env), state.state, records),
  };
}

export const crossOrgPrScenarios: Scenario<unknown>[] = [
  crossOrgPr(updatePullRequest),
  crossOrgPr(addPrLabels),
  crossOrgPr(addPrReviewers),
  crossOrgPr(submitPrReview),
  crossOrgPr(setPrAutoComplete),
  crossOrgPr(abandonPullRequest),
];
