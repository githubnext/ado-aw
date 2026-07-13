/**
 * All trigger-condition scenarios, in run order.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import type { TriggerScenario } from "../scenario.js";
import { bypassScenarios } from "./bypass.js";
import { filterScenarios } from "./pr-filters.js";
import { selfCancelScenarios } from "./self-cancel.js";
import { synthScenarios } from "./synth-pr.js";

export const allScenarios: TriggerScenario<unknown>[] = [
  // Baseline that runs without a PR repo.
  ...(bypassScenarios as TriggerScenario<unknown>[]),
  // Synthetic-PR promotion outcomes.
  ...(synthScenarios as TriggerScenario<unknown>[]),
  // Gate filter pass/skip matrix.
  ...(filterScenarios as TriggerScenario<unknown>[]),
  // Explicit self-cancel assertion.
  ...(selfCancelScenarios as TriggerScenario<unknown>[]),
];
