import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, it, vi } from "vitest";

import { runScenario } from "../runner.js";
import { updatePullRequestOversized } from "../scenarios/pr.js";
import { AdoRest } from "../ado-rest.js";
import { SkipError } from "../scenario.js";
import type { ExecutedRecord, Scenario, ScenarioContext } from "../scenario.js";

function fakeCtx(): ScenarioContext {
  return {
    orgUrl: "https://dev.azure.com/org/",
    project: "P",
    adoRepo: "agent-definitions",
    buildId: "1",
    token: "t",
    adoAwBin: "ado-aw",
    workDir: tmpdir(),
    rest: {} as ScenarioContext["rest"],
    log: () => {},
    prefix: (tool) => `ado-aw-det-1-${tool}`,
  };
}

/**
 * A scenario whose setup throws — the runner must never reach execute or run
 * cleanup. `flags.executed` trips if any post-setup phase runs; `flags.cleaned`
 * trips if cleanup runs. The runner swallows cleanup errors, so we surface the
 * violation via the returned flags rather than a throw.
 */
function guardScenario(
  setup: () => Promise<never>,
  flags: { executed: boolean; cleaned: boolean },
): Scenario<unknown> {
  return {
    tool: "guard",
    config: () => {
      flags.executed = true;
      return {};
    },
    setup,
    ndjson: async () => {
      flags.executed = true;
      return {};
    },
    assert: async () => {
      flags.executed = true;
    },
    cleanup: async () => {
      flags.cleaned = true;
    },
  };
}

describe("runScenario precondition handling", () => {
  it("marks SkipError from setup as skipped, not failed", async () => {
    const flags = { executed: false, cleaned: false };
    const scenario = guardScenario(async () => {
      throw new SkipError("no wiki");
    }, flags);
    const res = await runScenario(fakeCtx(), scenario);
    expect(res.ok).toBe(true);
    expect(res.skipped).toBe(true);
    expect(res.phase).toBe("skipped");
    expect(res.message).toBe("no wiki");
    expect(flags.executed).toBe(false);
    expect(flags.cleaned).toBe(false);
  });

  it("records a setup failure without reaching execute or cleanup", async () => {
    const flags = { executed: false, cleaned: false };
    const scenario = guardScenario(async () => {
      throw new Error("boom");
    }, flags);
    const res = await runScenario(fakeCtx(), scenario);
    expect(res.ok).toBe(false);
    expect(res.phase).toBe("setup");
    expect(res.message).toBe("boom");
    expect(flags.executed).toBe(false);
    expect(flags.cleaned).toBe(false);
  });
});

describe("runScenario expected executor failures", () => {
  it("the oversized PR scenario checks unchanged state through assertFailure", async () => {
    const base = fakeCtx();
    const rest = new AdoRest({ orgUrl: base.orgUrl, project: base.project, token: "" });
    const ctx: ScenarioContext = { ...base, rest };
    const state = { repo: "repo", prId: 42, branch: "test" };
    const records: ExecutedRecord[] = [{
      name: "update_pull_request", status: "failed", error: "4000-unit limit",
    }];
    const normal = "preserved original body";
    const body = await import("../scenarios/common.js");
    const expected = body.detBody(ctx, "update-pull-request-oversized");
    const getPr = vi.spyOn(rest, "getPullRequest").mockResolvedValue({
      pullRequestId: 42, status: "active", title: "title", description: expected,
    });
    expect(updatePullRequestOversized.assertFailure).toBeDefined();
    await updatePullRequestOversized.assertFailure!(ctx, state, records[0]!, records);
    getPr.mockResolvedValue({
      pullRequestId: 42, status: "active", title: "title", description: normal,
    });
    await expect(updatePullRequestOversized.assertFailure!(ctx, state, records[0]!, records))
      .rejects.toThrow("changed the live description");
  });

  async function outcomeBinary(
    dir: string,
    status: string | null,
    error: string,
    mutate = false,
  ): Promise<string> {
    const bin = join(dir, "outcome.js");
    const records = status === null ? [] : [{ name: "update_pull_request", status, error }];
    await writeFile(bin, `
const fs = require("node:fs");
const path = require("node:path");
const out = process.argv[process.argv.indexOf("--safe-output-dir") + 1];
if (${mutate}) fs.writeFileSync(path.join(out, "unexpected-write"), "changed");
fs.writeFileSync(path.join(out, "safe-outputs-executed.ndjson"), ${JSON.stringify(records.map((record) => JSON.stringify(record)).join("\n") + "\n")});
`, "utf8");
    return bin;
  }

  it.each([false, true])("checks postconditions after expected failure (mutated=%s)", async (mutated) => {
    const dir = await mkdtemp(join(tmpdir(), "ado-aw-negative-assert-"));
    try {
      const bin = await outcomeBinary(dir, "failed", "body too long", mutated);
      const flags = { failure: false, success: false, post: false, cleanup: false };
      const scenario: Scenario<unknown> = {
        id: "negative-case", tool: "update-pull-request",
        config: () => ({}), setup: async () => ({}), ndjson: async () => ({}),
        expectedFailure: { error: /too long/ },
        assertFailure: async (_ctx, _state, record, records) => {
          flags.failure = true;
          expect(record.status).toBe("failed");
          expect(records).toHaveLength(1);
          if (existsSync(join(dir, "negative-case", "out", "unexpected-write"))) {
            throw new Error("unexpected mutation");
          }
        },
        assert: async () => { flags.success = true; },
        postExecute: async () => { flags.post = true; },
        cleanup: async () => { flags.cleanup = true; },
      };
      const result = await runScenario({ ...fakeCtx(), adoAwBin: bin, workDir: dir }, scenario);
      expect(result.ok).toBe(!mutated);
      if (mutated) expect(result).toMatchObject({ phase: "assert", message: "unexpected mutation" });
      expect(flags).toEqual({ failure: true, success: false, post: false, cleanup: true });
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });

  it.each([
    { status: "succeeded", error: "" },
    { status: "failed", error: "different error" },
    { status: "warning", error: "body too long" },
    { status: null, error: "" },
  ])("rejects an unmatched outcome $status/$error", async ({ status, error }) => {
    const dir = await mkdtemp(join(tmpdir(), "ado-aw-negative-outcome-"));
    try {
      const bin = await outcomeBinary(dir, status, error);
      const flags = { asserted: false, cleaned: false };
      const scenario: Scenario<unknown> = {
        tool: "update-pull-request", config: () => ({}),
        setup: async () => ({}), ndjson: async () => ({}),
        expectedFailure: { error: /too long/ },
        assertFailure: async () => { flags.asserted = true; },
        assert: async () => { flags.asserted = true; },
        cleanup: async () => { flags.cleaned = true; },
      };
      const result = await runScenario({ ...fakeCtx(), adoAwBin: bin, workDir: dir }, scenario);
      expect(result).toMatchObject({ ok: false, phase: "execute" });
      expect(result.message).toContain(status === null
        ? "no executed record"
        : "expected rejection was not observed");
      expect(result.message).not.toMatch(/EACCES|ENOENT|spawn/);
      expect(flags).toEqual({ asserted: false, cleaned: true });
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });

  it("passes an expected executor rejection without running assertions", async () => {
    const dir = await mkdtemp(join(tmpdir(), "ado-aw-runner-test-"));
    try {
      const bin = join(dir, "fake-ado-aw.js");
      await writeFile(
        bin,
        `#!/usr/bin/env node
const fs = require("node:fs");
const path = require("node:path");
const out = process.argv[process.argv.indexOf("--safe-output-dir") + 1];
fs.writeFileSync(path.join(out, "safe-outputs-executed.ndjson"), JSON.stringify({
  name: "upload_pipeline_artifact",
  status: "failed",
  error: "SHA-256 mismatch: expected 0000, got abcd",
}) + "\\n");
`,
        { encoding: "utf8", mode: 0o755 },
      );

      let asserted = false;
      let cleaned = false;
      const scenario: Scenario<unknown> = {
        id: "upload-pipeline-artifact-sha-mismatch",
        tool: "upload-pipeline-artifact",
        config: () => ({}),
        setup: async () => ({}),
        ndjson: async () => ({}),
        expectedFailure: { status: "failed", error: /SHA-256 mismatch/ },
        assert: async () => {
          asserted = true;
        },
        cleanup: async () => {
          cleaned = true;
        },
      };

      const res = await runScenario({ ...fakeCtx(), adoAwBin: bin, workDir: dir }, scenario);

      expect(res).toMatchObject({ ok: true, tool: "upload-pipeline-artifact-sha-mismatch" });
      expect(asserted).toBe(false);
      expect(cleaned).toBe(true);
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });
});

/**
 * `priorEntries` lets one scenario stage extra safe-output lines ahead of its
 * primary entry inside a single `ado-aw execute` run. These tests use a fake
 * binary that echoes the staged NDJSON back as executed records, so they
 * exercise the real staging/ordering/validation path without a real executor.
 */
describe("runScenario prior entries", () => {
  /**
   * Fake `ado-aw` that turns every staged input line into an executed record,
   * preserving order. `statuses` overrides the status for a given tool.
   */
  async function writeEchoBin(
    dir: string,
    statuses: Record<string, string> = {},
    statusesByIndex: Record<number, string> = {},
  ): Promise<string> {
    const bin = join(dir, "echo-ado-aw.js");
    await writeFile(
      bin,
      `#!/usr/bin/env node
const fs = require("node:fs");
const path = require("node:path");
const out = process.argv[process.argv.indexOf("--safe-output-dir") + 1];
const statuses = ${JSON.stringify(statuses)};
const statusesByIndex = ${JSON.stringify(statusesByIndex)};
const lines = fs.readFileSync(path.join(out, "safe_outputs.ndjson"), "utf8")
  .split(/\\r?\\n/).filter((l) => l.trim());
const records = lines.map((l, i) => {
  const parsed = JSON.parse(l);
  return {
    name: parsed.name.replaceAll("-", "_"),
    status: statusesByIndex[i] ?? statuses[parsed.name] ?? "succeeded",
    error: statusesByIndex[i] || statuses[parsed.name] ? "synthetic prior failure" : null,
    result: { order: i, tool: parsed.name },
  };
});
fs.writeFileSync(
  path.join(out, "safe-outputs-executed.ndjson"),
  records.map((r) => JSON.stringify(r)).join("\\n") + "\\n",
);
`,
      { encoding: "utf8", mode: 0o755 },
    );
    return bin;
  }

  it.each(["succeeded", "failed"])("selects the second same-tool entry when its status is %s", async (status) => {
    const dir = await mkdtemp(join(tmpdir(), "ado-aw-same-tool-"));
    try {
      const bin = await writeEchoBin(dir, {}, { 1: status });
      let asserted = false;
      let cleaned = false;
      const scenario: Scenario<unknown> = {
        tool: "update-pull-request", config: () => ({ max: 2 }),
        setup: async () => ({}), ndjson: async () => ({ body: "second" }),
        priorEntries: async () => [{ tool: "update-pull-request", config: { max: 2 }, entry: { body: "first" } }],
        assert: async (_ctx, _state, record) => {
          asserted = true;
          expect(record.result?.order).toBe(1);
        },
        cleanup: async () => { cleaned = true; },
      };
      const result = await runScenario({ ...fakeCtx(), adoAwBin: bin, workDir: dir }, scenario);
      expect(result.ok).toBe(status === "succeeded");
      expect(asserted).toBe(status === "succeeded");
      expect(cleaned).toBe(true);
      if (status === "failed") expect(result.phase).toBe("execute");
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });

  it("does not substitute a successful prior record for a missing same-tool primary", async () => {
    const dir = await mkdtemp(join(tmpdir(), "ado-aw-missing-primary-"));
    try {
      const bin = join(dir, "only-prior.js");
      await writeFile(bin, `
const fs = require("node:fs");
const path = require("node:path");
const out = process.argv[process.argv.indexOf("--safe-output-dir") + 1];
fs.writeFileSync(path.join(out, "safe-outputs-executed.ndjson"), JSON.stringify({
  name: "update_pull_request", status: "succeeded", result: {order: 0}
}) + "\\n");
`, "utf8");
      let cleaned = false;
      const scenario: Scenario<unknown> = {
        tool: "update-pull-request", config: () => ({ max: 2 }),
        setup: async () => ({}), ndjson: async () => ({ body: "second" }),
        priorEntries: async () => [{ tool: "update-pull-request", config: { max: 2 }, entry: { body: "first" } }],
        assert: async () => { throw new Error("primary record is absent"); },
        cleanup: async () => { cleaned = true; },
      };
      const result = await runScenario({ ...fakeCtx(), adoAwBin: bin, workDir: dir }, scenario);
      expect(result).toMatchObject({ ok: false, phase: "execute" });
      expect(result.message).toContain("no executed record");
      expect(cleaned).toBe(true);
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });

  function handoffScenario(
    onAssert: (records: ExecutedRecord[]) => void,
    onCleanup: (records: ExecutedRecord[] | undefined) => void = () => {},
  ): Scenario<unknown> {
    return {
      id: "prior-entry-handoff",
      tool: "set-github-issue-type",
      config: () => ({ "target-repo": "o/r" }),
      setup: async () => ({}),
      priorEntries: async () => [
        { tool: "create-github-issue", config: { "target-repo": "o/r" }, entry: { title: "t" } },
      ],
      ndjson: async () => ({ issue_number: "#aw_x1" }),
      assert: async (_ctx, _state, _record, records) => onAssert(records),
      cleanup: async (_ctx, _state, records) => onCleanup(records),
    };
  }

  it("writes prior entries before the primary entry and exposes all records to assert", async () => {
    const dir = await mkdtemp(join(tmpdir(), "ado-aw-runner-prior-"));
    try {
      const bin = await writeEchoBin(dir);
      let seen: ExecutedRecord[] = [];
      const res = await runScenario(
        { ...fakeCtx(), adoAwBin: bin, workDir: dir },
        handoffScenario((records) => {
          seen = records;
        }),
      );
      expect(res.ok).toBe(true);
      // Ordering matters: the producer must execute first, otherwise the
      // in-process temporary-id registry has nothing to resolve.
      expect(seen.map((r) => r.name)).toEqual([
        "create_github_issue",
        "set_github_issue_type",
      ]);
      expect(seen[0]!.result!.order).toBe(0);
      expect(seen[1]!.result!.order).toBe(1);

      // Both tools must appear in the rendered front matter, or the executor
      // would report "not configured for this workflow".
      const source = await readFile(join(dir, "prior-entry-handoff", "source.md"), "utf8");
      expect(source).toContain('"create-github-issue"');
      expect(source).toContain('"set-github-issue-type"');
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });

  it("fails in the execute phase when a prior entry did not succeed", async () => {
    const dir = await mkdtemp(join(tmpdir(), "ado-aw-runner-prior-fail-"));
    try {
      const bin = await writeEchoBin(dir, { "create-github-issue": "failed" });
      let asserted = false;
      const res = await runScenario(
        { ...fakeCtx(), adoAwBin: bin, workDir: dir },
        handoffScenario(() => {
          asserted = true;
        }),
      );
      expect(res.ok).toBe(false);
      expect(res.phase).toBe("execute");
      expect(res.message).toContain("prior entry 'create-github-issue'");
      // The prerequisite failure must not be reported as an assertion failure.
      expect(asserted).toBe(false);
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });

  it("exposes prior records to cleanup when the primary entry fails", async () => {
    const dir = await mkdtemp(join(tmpdir(), "ado-aw-runner-primary-fail-"));
    try {
      const bin = await writeEchoBin(dir, { "set-github-issue-type": "failed" });
      let asserted = false;
      let cleanedRecords: ExecutedRecord[] | undefined;
      const res = await runScenario(
        { ...fakeCtx(), adoAwBin: bin, workDir: dir },
        handoffScenario(
          () => {
            asserted = true;
          },
          (records) => {
            cleanedRecords = records;
          },
        ),
      );

      expect(res.ok).toBe(false);
      expect(res.phase).toBe("execute");
      expect(res.message).toContain("executor reported status='failed'");
      expect(asserted).toBe(false);
      expect(cleanedRecords?.map((record) => record.name)).toEqual([
        "create_github_issue",
        "set_github_issue_type",
      ]);
      expect(cleanedRecords?.[0]?.status).toBe("succeeded");
      expect(cleanedRecords?.[1]?.status).toBe("failed");
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });

  it("fails in the execute phase when a prior entry produced no record", async () => {
    const dir = await mkdtemp(join(tmpdir(), "ado-aw-runner-prior-missing-"));
    try {
      const bin = join(dir, "drop-prior.js");
      await writeFile(
        bin,
        `#!/usr/bin/env node
const fs = require("node:fs");
const path = require("node:path");
const out = process.argv[process.argv.indexOf("--safe-output-dir") + 1];
fs.writeFileSync(path.join(out, "safe-outputs-executed.ndjson"), JSON.stringify({
  name: "set_github_issue_type",
  status: "succeeded",
  result: {},
}) + "\\n");
`,
        { encoding: "utf8", mode: 0o755 },
      );
      const res = await runScenario(
        { ...fakeCtx(), adoAwBin: bin, workDir: dir },
        handoffScenario(() => {}),
      );
      expect(res.ok).toBe(false);
      expect(res.phase).toBe("execute");
      expect(res.message).toContain("produced no executed record");
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });

  it("requires one executed record for each repeated prior tool occurrence", async () => {
    const dir = await mkdtemp(join(tmpdir(), "ado-aw-runner-prior-repeat-"));
    try {
      const bin = join(dir, "drop-second-prior.js");
      await writeFile(
        bin,
        `#!/usr/bin/env node
const fs = require("node:fs");
const path = require("node:path");
const out = process.argv[process.argv.indexOf("--safe-output-dir") + 1];
fs.writeFileSync(path.join(out, "safe-outputs-executed.ndjson"), [
  { name: "create_github_issue", status: "succeeded", result: { number: 1 } },
  { name: "link_github_sub_issue", status: "succeeded", result: {} },
].map(JSON.stringify).join("\\n") + "\\n");
`,
        { encoding: "utf8", mode: 0o755 },
      );
      const scenario: Scenario<unknown> = {
        id: "repeated-prior",
        tool: "link-github-sub-issue",
        config: () => ({ "target-repo": "o/r" }),
        setup: async () => ({}),
        priorEntries: async () => [
          { tool: "create-github-issue", config: {}, entry: { temporary_id: "#aw_parent" } },
          { tool: "create-github-issue", config: {}, entry: { temporary_id: "#aw_sub" } },
        ],
        ndjson: async () => ({
          parent_issue_number: "#aw_parent",
          sub_issue_number: "#aw_sub",
        }),
        assert: async () => {},
        cleanup: async () => {},
      };
      const res = await runScenario(
        { ...fakeCtx(), adoAwBin: bin, workDir: dir },
        scenario,
      );
      expect(res.ok).toBe(false);
      expect(res.phase).toBe("execute");
      expect(res.message).toContain("occurrence 2");
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });
});

/**
 * `postExecute` runs a post-Stage-3 consumer (the Conclusion reporter) against
 * the manifest the executor just wrote, before `assert`. These tests pin the
 * ordering, the safe-output dir it is handed, and the failure/skip handling.
 */
describe("runScenario post-execute phase", () => {
  /** Fake `ado-aw` that reports the primary tool as succeeded. */
  async function writeOkBin(dir: string): Promise<string> {
    const bin = join(dir, "ok-ado-aw.js");
    await writeFile(
      bin,
      `#!/usr/bin/env node
const fs = require("node:fs");
const path = require("node:path");
const out = process.argv[process.argv.indexOf("--safe-output-dir") + 1];
fs.writeFileSync(
  path.join(out, "safe-outputs-executed.ndjson"),
  JSON.stringify({ name: "noop", status: "succeeded", result: {} }) + "\\n",
);
`,
      { encoding: "utf8", mode: 0o755 },
    );
    return bin;
  }

  function postExecuteScenario(
    postExecute: Scenario<unknown>["postExecute"],
    order: string[],
  ): Scenario<unknown> {
    return {
      id: "post-execute",
      tool: "noop",
      config: () => ({}),
      setup: async () => ({}),
      ndjson: async () => ({}),
      postExecute,
      assert: async () => {
        order.push("assert");
      },
      cleanup: async () => {
        order.push("cleanup");
      },
    };
  }

  it("runs before assert and receives the executor's safe-output dir and records", async () => {
    const dir = await mkdtemp(join(tmpdir(), "ado-aw-runner-post-"));
    try {
      const bin = await writeOkBin(dir);
      const order: string[] = [];
      let seenDir = "";
      let seenRecords: ExecutedRecord[] = [];
      const res = await runScenario(
        { ...fakeCtx(), adoAwBin: bin, workDir: dir },
        postExecuteScenario(async (_ctx, _state, run) => {
          order.push("post-execute");
          seenDir = run.safeOutputDir;
          seenRecords = run.records;
          // The executed manifest must be readable from the handed-over dir.
          await readFile(join(run.safeOutputDir, "safe-outputs-executed.ndjson"), "utf8");
        }, order),
      );

      expect(res.ok).toBe(true);
      expect(order).toEqual(["post-execute", "assert", "cleanup"]);
      expect(seenDir).toBe(join(dir, "post-execute", "out"));
      expect(seenRecords.map((r) => r.name)).toEqual(["noop"]);
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });

  it("records a post-execute failure without running assert, but still cleans up", async () => {
    const dir = await mkdtemp(join(tmpdir(), "ado-aw-runner-post-"));
    try {
      const bin = await writeOkBin(dir);
      const order: string[] = [];
      const res = await runScenario(
        { ...fakeCtx(), adoAwBin: bin, workDir: dir },
        postExecuteScenario(async () => {
          throw new Error("conclusion.js exited 3");
        }, order),
      );

      expect(res.ok).toBe(false);
      expect(res.phase).toBe("post-execute");
      expect(res.message).toBe("conclusion.js exited 3");
      expect(order).toEqual(["cleanup"]);
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });

  it("treats a SkipError from post-execute as a skip", async () => {
    const dir = await mkdtemp(join(tmpdir(), "ado-aw-runner-post-"));
    try {
      const bin = await writeOkBin(dir);
      const order: string[] = [];
      const res = await runScenario(
        { ...fakeCtx(), adoAwBin: bin, workDir: dir },
        postExecuteScenario(async () => {
          throw new SkipError("conclusion bundle not built");
        }, order),
      );

      expect(res).toMatchObject({ ok: true, skipped: true, phase: "skipped" });
      expect(order).toEqual(["cleanup"]);
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });
});
