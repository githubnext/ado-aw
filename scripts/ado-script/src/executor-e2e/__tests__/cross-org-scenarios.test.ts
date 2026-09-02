import { afterEach, describe, expect, it, vi } from "vitest";

import type { ScenarioContext } from "../scenario.js";
import { SkipError } from "../scenario.js";
import {
  crossOrgSource,
  resolveCrossOrgEnv,
} from "../scenarios/cross-org.js";

function fakeCtx(): ScenarioContext {
  return {
    orgUrl: "https://dev.azure.com/current/",
    project: "Current",
    adoRepo: "repo",
    buildId: "77",
    token: "token",
    adoAwBin: "ado-aw",
    workDir: "/tmp",
    rest: {} as ScenarioContext["rest"],
    log: () => {},
    prefix: (tool) => `ado-aw-det-77-${tool}`,
  };
}

afterEach(() => {
  vi.unstubAllEnvs();
});

describe("cross-org executor environment", () => {
  it("skips when the pre-provisioned infrastructure is absent", () => {
    expect(() => resolveCrossOrgEnv(fakeCtx())).toThrow(SkipError);
  });

  it("builds one exact repository and write scope from environment", () => {
    vi.stubEnv("EXECUTOR_E2E_CROSS_ORG_ORGANIZATION", "other-org");
    vi.stubEnv("EXECUTOR_E2E_CROSS_ORG_PROJECT", "Other Project");
    vi.stubEnv("EXECUTOR_E2E_CROSS_ORG_REPOSITORY", "target-repo");
    vi.stubEnv("EXECUTOR_E2E_CROSS_ORG_ENDPOINT", "ado-write");
    vi.stubEnv("EXECUTOR_E2E_CROSS_ORG_TOKEN", "entra-token");

    const env = resolveCrossOrgEnv(fakeCtx());
    const source = crossOrgSource(env);

    expect(env.orgUrl).toBe("https://dev.azure.com/other-org/");
    expect(source.repositories).toEqual([{
      name: "Other Project/target-repo",
      alias: "cross-org-target",
      organization: "other-org",
      endpoint: "ado-write",
    }]);
    expect(source.writePermissions).toEqual({
      serviceConnection: "ado-write",
      connectionType: "azureDevOps",
      allow: [{
        organization: "other-org",
        projects: [{
          project: "Other Project",
          repositories: ["target-repo"],
        }],
      }],
    });
  });

  it("treats unexpanded pipeline macros as absent", () => {
    vi.stubEnv(
      "EXECUTOR_E2E_CROSS_ORG_ORGANIZATION",
      "$(EXECUTOR_E2E_CROSS_ORG_ORGANIZATION)",
    );
    expect(() => resolveCrossOrgEnv(fakeCtx())).toThrow(SkipError);
  });
});
