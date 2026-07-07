/**
 * Shared helpers for executor E2E scenarios.
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import type { ExecutedRecord, ScenarioContext } from "../scenario.js";
import { SkipError } from "../scenario.js";

/** Read a numeric field from the executor's success `result` payload. */
export function numResult(record: ExecutedRecord, key: string): number {
  const value = record.result?.[key];
  // Require an actual number: Number(null/""/false/[]) all coerce to a finite
  // 0, which would silently pass an invalid/missing id through the assertion.
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new Error(`executor result.${key} is not a number (got ${JSON.stringify(value)})`);
  }
  return value;
}

/** Read a string field from the executor's success `result` payload. */
export function strResult(record: ExecutedRecord, key: string): string {
  const value = record.result?.[key];
  if (typeof value !== "string") {
    throw new Error(`executor result.${key} is not a string (got ${JSON.stringify(value)})`);
  }
  return value;
}

/** A body that comfortably clears the various >10/>30-char minimums. */
export function detBody(ctx: ScenarioContext, tool: string): string {
  return `Deterministic executor E2E exercising ${tool} for build ${ctx.buildId}. Safe to delete.`;
}

/**
 * Resolve the short name of a repo's default branch (e.g. `main`), stripping
 * the `refs/heads/` prefix. Shared by the git and pr scenarios so a fix here
 * reaches both.
 */
export async function defaultBranchShortName(
  ctx: ScenarioContext,
  repo: string,
): Promise<string> {
  const info = await ctx.rest.getRepository(repo);
  return (info.defaultBranch ?? "refs/heads/main").replace(/^refs\/heads\//, "");
}

/** Read a required env var, or SkipError the scenario when it is absent. */
export function requireEnv(name: string, tool: string): string {
  const value = process.env[name]?.trim();
  if (!value) {
    throw new SkipError(`${tool}: ${name} is not set; supply it to enable this scenario`);
  }
  return value;
}

/**
 * Runs a series of *independent* teardown steps, guaranteeing every step is
 * attempted even if an earlier one throws.
 *
 * This exists to kill a recurring cleanup bug: writing teardown as sequential
 * awaits — `await abandonPr(); await deleteBranch();` — silently skips
 * `deleteBranch()` whenever `abandonPr()` rejects, leaking the branch. Wrapping
 * each in an ad-hoc `.catch(() => {})` fixes the leak but swallows the failure.
 *
 * Instead, register each step and call `run()`: every step runs, failures are
 * collected, and a single aggregated error is thrown at the end so a genuinely
 * broken teardown still surfaces (the runner logs it as a cleanup WARNING)
 * without any step being skipped.
 *
 * ```ts
 * await new Teardown()
 *   .add("abandon PR", () => ctx.rest.abandonPullRequest(repo, prId))
 *   .add("delete branch", () => ctx.rest.deleteRef(repo, `refs/heads/${branch}`))
 *   .run();
 * ```
 */
export class Teardown {
  private readonly steps: { label: string; fn: () => Promise<unknown> }[] = [];

  /** Register one independent teardown step. Returns `this` for chaining. */
  add(label: string, fn: () => Promise<unknown>): this {
    this.steps.push({ label, fn });
    return this;
  }

  /**
   * Run every registered step in order. All steps are attempted regardless of
   * individual failures; if any threw, a single aggregated error is thrown
   * after the last step completes.
   */
  async run(): Promise<void> {
    const failures: string[] = [];
    for (const { label, fn } of this.steps) {
      try {
        await fn();
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        failures.push(`${label}: ${message}`);
      }
    }
    if (failures.length > 0) {
      throw new Error(`teardown had ${failures.length} failure(s): ${failures.join("; ")}`);
    }
  }
}
