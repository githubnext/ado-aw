import { describe, expect, it } from "vitest";
import { readText, runImportSource, withScratchDir, writeFixture } from "./helpers.js";

describe("runtime-import optional markers", () => {
  it("drops a missing optional marker", () => {
    withScratchDir("optional-marker", (dir) => {
      const target = writeFixture(
        dir,
        "prompt.md",
        "top\n{{#runtime-import? ./missing.md}}\nbottom\n",
      );

      const result = runImportSource(target);

      expect(result.status).toBe(0);
      expect(result.stdout).toBe("");
      expect(result.stderr).toBe("");
      expect(readText(target)).toBe("top\n\nbottom\n");
    });
  });

  it("fails when a required marker is missing", () => {
    withScratchDir("required-missing", (dir) => {
      const original = "top\n{{#runtime-import ./missing.md}}\nbottom\n";
      const target = writeFixture(dir, "prompt.md", original);

      const result = runImportSource(target);

      expect(result.status).toBe(1);
      expect(result.stderr).toBe("");
      expect(result.stdout).toBe(
        "##vso[task.logissue type=error]runtime-import: file not found: ./missing.md\n",
      );
      expect(readText(target)).toBe(original);
    });
  });
});
