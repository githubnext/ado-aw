import { describe, expect, it } from "vitest";
import { readText, runImportSource, withScratchDir, writeFixture } from "./helpers.js";

describe("runtime-import path resolution", () => {
  it("rejects POSIX-absolute snippet paths", () => {
    // Defence in depth: the agent VM has privileged material at
    // well-known paths (e.g. `/tmp/awf-tools/staging/...`). Author
    // markers must NOT be able to read those even if the design ever
    // changes to multi-pass.
    withScratchDir("absolute-posix", (dir) => {
      const snippet = writeFixture(dir, "shared/absolute.md", "ABSOLUTE\n");
      const target = writeFixture(
        dir,
        "prompt.md",
        `value={{#runtime-import ${snippet}}}\n`,
      );

      const result = runImportSource(target);

      expect(result.status).toBe(1);
      expect(result.stdout).toContain("absolute paths are not allowed");
      // Target file MUST NOT be written when the resolver fails.
      expect(readText(target)).toBe(`value={{#runtime-import ${snippet}}}\n`);
    });
  });

  it("rejects Windows drive-letter absolute paths regardless of host OS", () => {
    withScratchDir("absolute-drive", (dir) => {
      const target = writeFixture(
        dir,
        "prompt.md",
        "{{#runtime-import C:\\Users\\runner\\secret.txt}}\n",
      );

      const result = runImportSource(target);

      expect(result.status).toBe(1);
      expect(result.stdout).toContain("absolute paths are not allowed");
    });
  });

  it("rejects UNC absolute paths", () => {
    withScratchDir("absolute-unc", (dir) => {
      const target = writeFixture(
        dir,
        "prompt.md",
        "{{#runtime-import \\\\server\\share\\file.md}}\n",
      );

      const result = runImportSource(target);

      expect(result.status).toBe(1);
      expect(result.stdout).toContain("absolute paths are not allowed");
    });
  });

  it("resolves relative paths against dirname(target) by default", () => {
    // When `--base` is not passed, fall back to `dirname(argv[2])` so
    // standalone (non-pipeline) invocations behave predictably.
    withScratchDir("relative-default-base", (dir) => {
      writeFixture(dir, "snippet.md", "SIBLING\n");
      const target = writeFixture(
        dir,
        "prompt.md",
        "start\n{{#runtime-import ./snippet.md}}\nend\n",
      );

      const result = runImportSource(target);

      expect(result.status).toBe(0);
      expect(readText(target)).toBe("start\nSIBLING\n\nend\n");
    });
  });

  it("resolves relative paths against the --base argument when provided", () => {
    // The pipeline-emitted resolver step passes
    // `--base "$(Build.SourcesDirectory)"`. The compiler-emitted
    // marker is trigger-repo-relative (e.g. `agents/foo.md`), so it
    // must resolve against the base, NOT against `dirname(target)`
    // (which would be `/tmp/awf-tools/`).
    withScratchDir("relative-with-base", (dir) => {
      writeFixture(dir, "sources/agents/foo.md", "AGENT_BODY\n");
      const target = writeFixture(
        dir,
        "prompt.md",
        "{{#runtime-import agents/foo.md}}\n",
      );

      const result = runImportSource(target, {
        base: `${dir}/sources`,
      });

      expect(result.status).toBe(0);
      expect(readText(target)).toBe("AGENT_BODY\n\n");
    });
  });
});
