/**
 * Shared helpers for executor E2E scenarios.
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import type { ExecutedRecord, ScenarioContext } from "../scenario.js";
import { SkipError } from "../scenario.js";

/** Read a numeric field from the executor's success `result` payload. */
export function numResult(record: ExecutedRecord, key: string): number {
  const value = record.result?.[key];
  const n = typeof value === "number" ? value : Number(value);
  if (!Number.isFinite(n)) {
    throw new Error(`executor result.${key} is not a number (got ${JSON.stringify(value)})`);
  }
  return n;
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
