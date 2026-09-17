import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import { CONCLUSION_BUNDLE_ENV, resolveConclusionBundle, runConclusion } from "../conclusion-cli.js";
import { SkipError } from "../scenario.js";

const originalBundle = process.env[CONCLUSION_BUNDLE_ENV];

afterEach(() => {
  if (originalBundle === undefined) delete process.env[CONCLUSION_BUNDLE_ENV];
  else process.env[CONCLUSION_BUNDLE_ENV] = originalBundle;
});

describe("resolveConclusionBundle", () => {
  it("skips the scenario when the bundle env var is unset", () => {
    delete process.env[CONCLUSION_BUNDLE_ENV];
    expect(() => resolveConclusionBundle()).toThrow(SkipError);
  });

  it("skips the scenario when the configured bundle does not exist", () => {
    process.env[CONCLUSION_BUNDLE_ENV] = join(tmpdir(), "definitely-missing-conclusion.js");
    expect(() => resolveConclusionBundle()).toThrow(SkipError);
  });
});

describe("runConclusion", () => {
  it("passes the safe-output dir, pipeline name and per-tool config to the bundle", async () => {
    const dir = await mkdtemp(join(tmpdir(), "ado-aw-conclusion-cli-"));
    try {
      // Fake bundle: echo the env the harness handed it, so the test pins the
      // env-var contract shared with the compiler-generated Conclusion job.
      const bundle = join(dir, "fake-conclusion.js");
      await writeFile(
        bundle,
        `const keys = ["AW_SAFE_OUTPUT_DIR","AW_PIPELINE_NAME","AW_AGENT_RESULT",` +
          `"AW_NOOP_TITLE_PREFIX","SYSTEM_TEAMPROJECT","SYSTEM_COLLECTIONURI","BUILD_BUILDID"];\n` +
          `console.log(JSON.stringify(Object.fromEntries(keys.map((k) => [k, process.env[k]]))));\n`,
        "utf8",
      );
      process.env[CONCLUSION_BUNDLE_ENV] = bundle;

      const result = await runConclusion({
        safeOutputDir: join(dir, "out"),
        pipelineName: "ado-aw-det-1-conclusion-noop",
        orgUrl: "https://dev.azure.com/org/",
        project: "P",
        token: "t",
        buildId: "1",
        config: { AW_NOOP_TITLE_PREFIX: "[prefix]" },
        log: () => {},
      });

      expect(JSON.parse(result.stdout.trim())).toEqual({
        AW_SAFE_OUTPUT_DIR: join(dir, "out"),
        AW_PIPELINE_NAME: "ado-aw-det-1-conclusion-noop",
        AW_AGENT_RESULT: "Succeeded",
        AW_NOOP_TITLE_PREFIX: "[prefix]",
        SYSTEM_TEAMPROJECT: "P",
        SYSTEM_COLLECTIONURI: "https://dev.azure.com/org/",
        BUILD_BUILDID: "1",
      });
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });

  it("fails when the bundle exits non-zero", async () => {
    const dir = await mkdtemp(join(tmpdir(), "ado-aw-conclusion-cli-"));
    try {
      const bundle = join(dir, "crashing-conclusion.js");
      await writeFile(bundle, `console.error("boom");\nprocess.exit(3);\n`, "utf8");
      process.env[CONCLUSION_BUNDLE_ENV] = bundle;

      await expect(
        runConclusion({
          safeOutputDir: dir,
          pipelineName: "p",
          orgUrl: "https://dev.azure.com/org/",
          project: "P",
          token: "t",
          buildId: "1",
          config: {},
          log: () => {},
        }),
      ).rejects.toThrow(/conclusion\.js exited 3/);
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });
});
