import { describe, expect, it, vi } from "vitest";

import type { ExecutedRecord, ScenarioContext } from "../scenario.js";
import { SkipError } from "../scenario.js";
import renderingCorpus from "../scenarios/markdown-rendering-corpus.json" with { type: "json" };
import {
  assignWorkItemTemporaryIdHandoff,
  createBugWorkItemRendering,
  createWorkItem,
  createWorkItemRendering,
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

describe("create-work-item rendering fidelity", () => {
  const adoExpected = renderingCorpus.ado_expected.join("\n");

  function renderingCtx(
    payload: {
      id: number;
      fields: Record<string, unknown>;
      multilineFieldsFormat?: Record<string, unknown>;
    },
    typeExists = true,
  ): ScenarioContext {
    return {
      ...fakeCtx(),
      rest: {
        getWorkItem: vi.fn(async () => payload),
        workItemTypeExists: vi.fn(async () => typeExists),
      } as unknown as ScenarioContext["rest"],
    };
  }

  const record: ExecutedRecord = {
    name: "create_work_item",
    status: "succeeded",
    result: { id: 42 },
  };

  it("registers both description-field scenarios", () => {
    const ids = workItemScenarios.map((scenario) => scenario.id ?? scenario.tool);
    expect(ids).toContain("create-work-item-rendering");
    expect(ids).toContain("create-work-item-rendering-bug");
  });

  it("proposes the raw (unsanitized) corpus so the executor does the sanitizing", async () => {
    const entry = await createWorkItemRendering.ndjson(fakeCtx(), {
      title: "ado-aw-det-77-create-work-item-rendering",
    });
    expect(entry.description).toBe(renderingCorpus.input.join("\n"));
    expect(createWorkItemRendering.config(fakeCtx(), { title: "t" })).toMatchObject({
      "work-item-type": "Task",
      "include-stats": false,
    });
    expect(createBugWorkItemRendering.config(fakeCtx(), { title: "t" })).toMatchObject({
      "work-item-type": "Bug",
    });
  });

  it("skips when the project does not define the work item type", async () => {
    await expect(
      createBugWorkItemRendering.setup(renderingCtx({ id: 42, fields: {} }, false)),
    ).rejects.toThrow(SkipError);
  });

  it.each(["Markdown", "markdown"])(
    "accepts the work-item Markdown format with %s casing",
    async (format) => {
      const state: { title: string; createdId?: number } = { title: "t" };
      await createWorkItemRendering.assert(
        renderingCtx({
          id: 42,
          fields: { "System.Description": adoExpected },
          multilineFieldsFormat: { "System.Description": format },
        }),
        state,
        record,
        [record],
      );
      expect(state.createdId).toBe(42);
    },
  );

  it("logs and continues when ADO omits the Markdown format metadata", async () => {
    const log = vi.fn();
    const ctx = {
      ...renderingCtx({
        id: 42,
        fields: { "System.Description": adoExpected },
      }),
      log,
    };

    await createWorkItemRendering.assert(
      ctx,
      { title: "t" },
      record,
      [record],
    );

    expect(log).toHaveBeenCalledWith(
      expect.stringContaining("ADO did not surface multilineFieldsFormat"),
    );
  });

  it("records the created id before failing an ADO golden mismatch", async () => {
    const state: { title: string; createdId?: number } = { title: "t" };
    await expect(
      createWorkItemRendering.assert(
        // Security-clean but not byte-identical: only the ADO golden catches it.
        renderingCtx({
          id: 42,
          fields: { "System.Description": adoExpected.replace("**bold**", "bold") },
        }),
        state,
        record,
        [record],
      ),
    ).rejects.toThrow(/does not match the ADO rendering golden/);
    expect(state.createdId).toBe(42);
  });

  it("fails when the field is not stored as Markdown", async () => {
    await expect(
      createWorkItemRendering.assert(
        renderingCtx({
          id: 42,
          fields: { "System.Description": adoExpected },
          multilineFieldsFormat: { "System.Description": "Html" },
        }),
        { title: "t" },
        record,
        [record],
      ),
    ).rejects.toThrow(/expected "Markdown"/);
  });

  it.each([
    ["script", "<script>alert(1)</script>", /still contains '<script'/],
    ["event handler", '<img src="x" onerror="alert(1)">', /still contains 'onerror='/],
    ["iframe", '<iframe src="https://example.test"></iframe>', /still contains '<iframe'/],
    ["javascript URL", "[click](javascript:alert(1))", /still contains a javascript: URL/],
  ])("fails when a denied %s survives ADO storage", async (_name, leaked, error) => {
    await expect(
      createWorkItemRendering.assert(
        renderingCtx({
          id: 42,
          fields: { "System.Description": `${adoExpected}\n${leaked}` },
        }),
        { title: "t" },
        record,
        [record],
      ),
    ).rejects.toThrow(error);
  });

  it("accepts the Bug repro-steps rendering path", async () => {
    const state: { title: string; createdId?: number } = { title: "t" };

    await createBugWorkItemRendering.assert(
      renderingCtx({
        id: 42,
        fields: { "Microsoft.VSTS.TCM.ReproSteps": adoExpected },
        multilineFieldsFormat: {
          "Microsoft.VSTS.TCM.ReproSteps": "markdown",
        },
      }),
      state,
      record,
      [record],
    );

    expect(state.createdId).toBe(42);
  });

  it("rejects a Bug response that omits the repro-steps field", async () => {
    await expect(
      createBugWorkItemRendering.assert(
        renderingCtx({ id: 42, fields: { "System.Description": adoExpected } }),
        { title: "t" },
        record,
        [record],
      ),
    ).rejects.toThrow(/has no string 'Microsoft\.VSTS\.TCM\.ReproSteps'/);
  });
});
