import { describe, expect, it } from "vitest";
import { readText, runImportSource, withScratchDir, writeFixture } from "./helpers.js";

describe("runtime-import single-pass behaviour", () => {
  it("does not re-expand nested markers introduced by imported snippets", () => {
    withScratchDir("single-pass", (dir) => {
      const target = writeFixture(
        dir,
        "prompt.md",
        "before\n{{#runtime-import ./snippet.md}}\nafter\n",
      );
      writeFixture(dir, "snippet.md", "nested {{#runtime-import ./inner.md}}\n");
      writeFixture(dir, "inner.md", "INNER\n");

      const result = runImportSource(target);

      expect(result.status).toBe(0);
      expect(readText(target)).toBe(
        "before\nnested {{#runtime-import ./inner.md}}\n\nafter\n",
      );
    });
  });
});
