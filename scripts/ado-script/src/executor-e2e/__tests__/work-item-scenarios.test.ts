import { describe, expect, it, vi } from "vitest";

import type { ExecutedRecord, ScenarioContext } from "../scenario.js";
import { SkipError } from "../scenario.js";
import {
  assignWorkItemTemporaryIdHandoff,
  createWorkItem,
  resolveWorkItemAssignee,
  workItemScenarios,
} from "../scenarios/work-item.js";

function fakeCtx(getWorkItem = vi.fn()): ScenarioContext {
  return {
    orgUrl: "https://dev.azure.com/org/",
    project: "P",
    adoRepo: "agent-definitions",
    buildId: "77",
    token: "ado-token",
    adoAwBin: "ado-aw",
    workDir: "/tmp",
    rest: { getWorkItem } as unknown as ScenarioContext["rest"],
    log: () => {},
    prefix: (tool) => `ado-aw-det-77-${tool}`,
  };
}

describe("resolveWorkItemAssignee", () => {
  it("prefers the explicit E2E identity", () => {
    expect(
      resolveWorkItemAssignee({
        E2E_WORK_ITEM_ASSIGNEE: "owner@example.com",
        BUILD_REQUESTEDFOREMAIL: "requester@example.com",
      } as NodeJS.ProcessEnv),
    ).toBe("owner@example.com");
  });

  it("falls back to the build requester", () => {
    expect(
      resolveWorkItemAssignee({
        BUILD_REQUESTEDFOREMAIL: "requester@example.com",
      } as NodeJS.ProcessEnv),
    ).toBe("requester@example.com");
  });

  it("skips unexpanded or reserved identities", () => {
    for (const env of [
      { E2E_WORK_ITEM_ASSIGNEE: "$(E2E_WORK_ITEM_ASSIGNEE)" },
      { E2E_WORK_ITEM_ASSIGNEE: "GitHub Copilot" },
    ]) {
      expect(() => resolveWorkItemAssignee(env as NodeJS.ProcessEnv)).toThrow(SkipError);
    }
  });

  it("skips when no identity is configured", () => {
    expect(() => resolveWorkItemAssignee({})).toThrow(SkipError);
  });
});

describe("create-work-item", () => {
  it("persists the deterministic temporary ID in standalone NDJSON", async () => {
    await expect(createWorkItem.ndjson(fakeCtx(), {})).resolves.toEqual(
      expect.objectContaining({ temporary_id: "#aw_wicreate" }),
    );
  });
});

describe("assign-work-item temporary-ID handoff", () => {
  it("is registered and stages create-work-item first", async () => {
    const ids = workItemScenarios.map((scenario) => scenario.id ?? scenario.tool);
    expect(ids).toContain("assign-work-item-temporary-id-handoff");

    const state = {
      assignee: "owner@example.com",
      title: "ado-aw-det-77-assign-work-item-temporary-id-handoff",
    };
    const prior = await assignWorkItemTemporaryIdHandoff.priorEntries!(
      fakeCtx(),
      state,
    );
    expect(prior).toEqual([
      expect.objectContaining({
        tool: "create-work-item",
        entry: expect.objectContaining({ temporary_id: "#aw_wiassign" }),
      }),
    ]);
    await expect(
      assignWorkItemTemporaryIdHandoff.ndjson(fakeCtx(), state),
    ).resolves.toEqual({
      work_item_id: "#aw_wiassign",
      assignee: "owner@example.com",
    });
  });

  it("asserts the resolved work item assignee and records cleanup state", async () => {
    const getWorkItem = vi.fn(async () => ({
      id: 42,
      fields: {
        "System.AssignedTo": {
          displayName: "Owner",
          uniqueName: "owner@example.com",
        },
      },
    }));
    const state: { assignee: string; title: string; createdId?: number } = {
      assignee: "OWNER@example.com",
      title: "ado-aw-det-77-assign-work-item-temporary-id-handoff",
    };
    const record: ExecutedRecord = {
      name: "assign_work_item",
      status: "succeeded",
      result: { id: 42, assignee: "owner@example.com" },
    };

    await assignWorkItemTemporaryIdHandoff.assert(
      fakeCtx(getWorkItem),
      state,
      record,
      [record],
    );
    expect(state.createdId).toBe(42);
    expect(getWorkItem).toHaveBeenCalledWith(42);
  });

  it("recovers the created item by title when assignment fails before assert", async () => {
    const findWorkItemByTitle = vi.fn(async () => 42);
    const deleteWorkItem = vi.fn(async () => {});
    const ctx = {
      ...fakeCtx(),
      rest: {
        findWorkItemByTitle,
        deleteWorkItem,
      } as unknown as ScenarioContext["rest"],
    };
    const state = {
      assignee: "owner@example.com",
      title: "ado-aw-det-77-assign-work-item-temporary-id-handoff",
    };

    await assignWorkItemTemporaryIdHandoff.cleanup(ctx, state);
    expect(findWorkItemByTitle).toHaveBeenCalledWith(state.title);
    expect(deleteWorkItem).toHaveBeenCalledWith(42);
  });
});
