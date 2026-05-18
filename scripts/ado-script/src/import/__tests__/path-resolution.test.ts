import { resolve } from "node:path";
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

  it("resolves relative paths against ADO_AW_IMPORT_BASE when set", () => {
    withScratchDir("base-override", (dir) => {
      writeFixture(dir, "imports/nested/snippet.md", "OVERRIDE\n");
      const target = writeFixture(
        dir,
        "prompt/prompt.md",
        "start\n{{#runtime-import nested/snippet.md}}\nend\n",
      );

      const result = runImportSource(target, {
        ADO_AW_IMPORT_BASE: resolve(dir, "imports"),
      });

      expect(result.status).toBe(0);
      expect(readText(target)).toBe("start\nOVERRIDE\n\nend\n");
    });
  });
});
