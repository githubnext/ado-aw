import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { runScenario } from "../runner.js";
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
  async function writeEchoBin(dir: string, statuses: Record<string, string> = {}): Promise<string> {
    const bin = join(dir, "echo-ado-aw.js");
    await writeFile(
      bin,
      `#!/usr/bin/env node
const fs = require("node:fs");
const path = require("node:path");
const out = process.argv[process.argv.indexOf("--safe-output-dir") + 1];
const statuses = ${JSON.stringify(statuses)};
const lines = fs.readFileSync(path.join(out, "safe_outputs.ndjson"), "utf8")
  .split(/\\r?\\n/).filter((l) => l.trim());
const records = lines.map((l, i) => {
  const parsed = JSON.parse(l);
  return {
    name: parsed.name.replaceAll("-", "_"),
    status: statuses[parsed.name] ?? "succeeded",
    error: statuses[parsed.name] ? "synthetic prior failure" : null,
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

  function handoffScenario(
    onAssert: (records: ExecutedRecord[]) => void,
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
      cleanup: async () => {},
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
