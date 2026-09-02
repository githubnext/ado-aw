import { afterEach, describe, expect, it, vi } from "vitest";

import { AdoRest } from "../ado-rest.js";
import type { ScenarioContext } from "../scenario.js";
import { SkipError } from "../scenario.js";
import {
  crossOrgSource,
  createCrossOrgBranch,
  createCrossOrgGitTag,
  resolveCrossOrgEnv,
  type CrossOrgEnv,
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

  describe("cross-org branch and tag scenarios", () => {
    function scenarioEnv(
      getRefObjectId: ReturnType<typeof vi.fn>,
      deleteRef = vi.fn(async () => {}),
    ): CrossOrgEnv {
      return {
        organization: "other-org",
        orgUrl: "https://dev.azure.com/other-org/",
        project: "Other Project",
        repository: "target-repo",
        alias: "cross-org-target",
        endpoint: "ado-write",
        token: "entra-token",
        rest: {
          getRefObjectId,
          deleteRef,
        } as unknown as AdoRest,
      };
    }

    it("builds and verifies a cross-org branch proposal", async () => {
      const getRefObjectId = vi.fn(async () => "a".repeat(40));
      const deleteRef = vi.fn(async () => {});
      const state = {
        env: scenarioEnv(getRefObjectId, deleteRef),
        branch: "ado-aw-det-77-cross-branch",
        base: "main",
      };

      const source = createCrossOrgBranch.source;
      if (!source) throw new Error("cross-org branch scenario source is required");
      await expect(source(fakeCtx(), state)).resolves.toEqual(crossOrgSource(state.env));
      expect(createCrossOrgBranch.config(fakeCtx(), state)).toEqual({
        "allowed-repositories": ["cross-org-target"],
        max: 1,
      });
      await expect(
        createCrossOrgBranch.ndjson(fakeCtx(), state),
      ).resolves.toEqual({
        branch_name: state.branch,
        source_branch: "main",
        repository: "cross-org-target",
      });
      await expect(
        createCrossOrgBranch.assert(
          fakeCtx(),
          state,
          {
            name: "create_branch",
            status: "succeeded",
          },
          [],
        ),
      ).resolves.toBeUndefined();
      expect(getRefObjectId).toHaveBeenCalledWith(
        "target-repo",
        `heads/${state.branch}`,
      );
      await createCrossOrgBranch.cleanup(fakeCtx(), state);
      expect(deleteRef).toHaveBeenCalledWith(
        "target-repo",
        `refs/heads/${state.branch}`,
      );
    });

    it("builds, verifies, and cleans up a cross-org tag proposal", async () => {
      const getRefObjectId = vi.fn(async () => "b".repeat(40));
      const deleteRef = vi.fn(async () => {});
      const state = {
        env: scenarioEnv(getRefObjectId, deleteRef),
        tag: "ado-aw-det-77-cross-tag",
      };

      const source = createCrossOrgGitTag.source;
      if (!source) throw new Error("cross-org tag scenario source is required");
      await expect(source(fakeCtx(), state)).resolves.toEqual(crossOrgSource(state.env));
      expect(createCrossOrgGitTag.config(fakeCtx(), state)).toEqual({
        "allowed-repositories": ["cross-org-target"],
        max: 1,
      });
      await expect(createCrossOrgGitTag.ndjson(fakeCtx(), state)).resolves.toEqual({
        tag_name: state.tag,
        message: expect.stringContaining("create-git-tag-cross-org"),
        repository: "cross-org-target",
      });
      await expect(
        createCrossOrgGitTag.assert(
          fakeCtx(),
          state,
          {
            name: "create_git_tag",
            status: "succeeded",
          },
          [],
        ),
      ).resolves.toBeUndefined();
      expect(getRefObjectId).toHaveBeenCalledWith(
        "target-repo",
        `tags/${state.tag}`,
      );
      await expect(
        createCrossOrgGitTag.env!(fakeCtx(), state),
      ).resolves.toEqual({
        SYSTEM_ACCESSTOKEN: "entra-token",
      });
      await createCrossOrgGitTag.cleanup(fakeCtx(), state);
      expect(deleteRef).toHaveBeenCalledWith(
        "target-repo",
        `refs/tags/${state.tag}`,
      );
    });

    it("fails verification when a cross-org tag is absent", async () => {
      const state = {
        env: scenarioEnv(vi.fn(async () => undefined)),
        tag: "ado-aw-det-77-cross-tag",
      };

      await expect(
        createCrossOrgGitTag.assert(
          fakeCtx(),
          state,
          {
            name: "create_git_tag",
            status: "succeeded",
          },
          [],
        ),
      ).rejects.toThrow(/was not created/);
    });

    it("fails verification when a cross-org branch is absent", async () => {
      const state = {
        env: scenarioEnv(vi.fn(async () => undefined)),
        branch: "ado-aw-det-77-cross-branch",
        base: "main",
      };

      await expect(
        createCrossOrgBranch.assert(
          fakeCtx(),
          state,
          {
            name: "create_branch",
            status: "succeeded",
          },
          [],
        ),
      ).rejects.toThrow(/was not created/);
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
