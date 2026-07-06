/**
 * Aggregated scenario registry for the deterministic executor E2E harness.
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import type { Scenario } from "../scenario.js";
import { buildScenarios } from "./build.js";
import { createPullRequestScenarios } from "./create-pull-request.js";
import { gitScenarios } from "./git.js";
import { prScenarios } from "./pr.js";
import { wikiScenarios } from "./wiki.js";
import { workItemScenarios } from "./work-item.js";

/** Every scenario, in a deterministic run order. */
export const allScenarios: Scenario<unknown>[] = [
  ...workItemScenarios,
  ...wikiScenarios,
  ...prScenarios,
  ...gitScenarios,
  ...buildScenarios,
  ...createPullRequestScenarios,
];
