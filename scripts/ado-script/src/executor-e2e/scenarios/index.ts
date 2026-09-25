/**
 * Aggregated scenario registry for the deterministic executor E2E harness.
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import type { Scenario } from "../scenario.js";
import { buildScenarios } from "./build.js";
import { conclusionScenarios } from "./conclusion.js";
import { createPullRequestScenarios } from "./create-pull-request.js";
import { crossOrgScenarios } from "./cross-org.js";
import { crossOrgPrScenarios } from "./pr-cross-org.js";
import { gitScenarios } from "./git.js";
import { githubIssueScenarios } from "./github-issue.js";
import { prScenarios } from "./pr.js";
import { prApiContractScenarios } from "./pr-api-contracts.js";
import { prOwnedCommentScenarios } from "./pr-comments.js";
import { signalScenarios } from "./signals.js";
import { wikiScenarios } from "./wiki.js";
import { workItemScenarios } from "./work-item.js";

/** Every scenario, in a deterministic run order. */
export const allScenarios: Scenario<unknown>[] = [
  ...signalScenarios,
  ...conclusionScenarios,
  ...workItemScenarios,
  ...wikiScenarios,
  ...prScenarios,
  ...prApiContractScenarios,
  ...prOwnedCommentScenarios,
  ...gitScenarios,
  ...crossOrgScenarios,
  ...crossOrgPrScenarios,
  ...buildScenarios,
  ...createPullRequestScenarios,
  ...githubIssueScenarios,
];
