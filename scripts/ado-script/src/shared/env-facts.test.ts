import { describe, it, expect, beforeEach, afterEach } from "vitest";
import {
  readEnvFact,
  stripRefPrefix,
  isPipelineVarFact,
  type FactKind,
} from "./env-facts.js";

const ALL: { fact: FactKind; env: string }[] = [
  { fact: "pr_title", env: "ADO_PR_TITLE" },
  { fact: "author_email", env: "ADO_AUTHOR_EMAIL" },
  { fact: "source_branch", env: "ADO_SOURCE_BRANCH" },
  { fact: "target_branch", env: "ADO_TARGET_BRANCH" },
  { fact: "commit_message", env: "ADO_COMMIT_MESSAGE" },
  { fact: "build_reason", env: "ADO_BUILD_REASON" },
  { fact: "triggered_by_pipeline", env: "ADO_TRIGGERED_BY_PIPELINE" },
  { fact: "triggering_branch", env: "ADO_TRIGGERING_BRANCH" },
];

describe("env-facts", () => {
  let saved: NodeJS.ProcessEnv;
  beforeEach(() => {
    saved = { ...process.env };
    for (const { env } of ALL) delete process.env[env];
  });
  afterEach(() => {
    process.env = saved;
  });

  it.each(ALL)("readEnvFact returns the value for $fact", ({ fact, env }) => {
    process.env[env] = "value-here";
    const expected = ["source_branch", "target_branch", "triggering_branch"].includes(
      fact
    )
      ? "value-here"
      : "value-here";
    expect(readEnvFact(fact)).toBe(expected);
  });

  it.each(ALL)("readEnvFact returns undefined when $fact env is empty", ({ fact, env }) => {
    process.env[env] = "";
    expect(readEnvFact(fact)).toBeUndefined();
  });

  it.each(ALL)("readEnvFact returns undefined when $fact env unset", ({ fact }) => {
    expect(readEnvFact(fact)).toBeUndefined();
  });

  it("strips refs/heads/ from source_branch", () => {
    process.env.ADO_SOURCE_BRANCH = "refs/heads/feature/x";
    expect(readEnvFact("source_branch")).toBe("feature/x");
  });

  it("strips refs/tags/ from target_branch", () => {
    process.env.ADO_TARGET_BRANCH = "refs/tags/v1.0";
    expect(readEnvFact("target_branch")).toBe("v1.0");
  });

  it("strips refs/pull/ from triggering_branch", () => {
    process.env.ADO_TRIGGERING_BRANCH = "refs/pull/42/merge";
    expect(readEnvFact("triggering_branch")).toBe("42/merge");
  });

  it("does NOT strip refs/heads/ from non-branch facts", () => {
    process.env.ADO_PR_TITLE = "refs/heads/title";
    expect(readEnvFact("pr_title")).toBe("refs/heads/title");
  });

  it("stripRefPrefix is a no-op for non-prefixed values", () => {
    expect(stripRefPrefix("main")).toBe("main");
  });

  it("isPipelineVarFact returns true for known kinds", () => {
    expect(isPipelineVarFact("pr_title")).toBe(true);
    expect(isPipelineVarFact("not_a_fact")).toBe(false);
  });
});
