import { describe, expect, it } from "vitest";
import { readText, runImportSource, withScratchDir, writeFixture } from "./helpers.js";

describe("runtime-import path resolution", () => {
  it("uses absolute snippet paths as-is", () => {
    withScratchDir("absolute-path", (dir) => {
      const snippet = writeFixture(dir, "shared/absolute.md", "ABSOLUTE\n");
      const target = writeFixture(
        dir,
        "prompt.md",
        `value={{#runtime-import ${snippet}}}\n`,
      );

      const result = runImportSource(target);

      expect(result.status).toBe(0);
      expect(readText(target)).toBe("value=ABSOLUTE\n\n");
    });
  });

  it("resolves relative paths against dirname(target)", () => {
    // The compiler always emits an absolute marker path, so this branch
    // is unreachable in pipeline use. The test pins the fallback so a
    // standalone invocation of `import.js` (e.g. local dev or future
    // callers) behaves predictably.
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
});
