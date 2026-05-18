import { describe, expect, it } from "vitest";
import { readText, runImportSource, withScratchDir, writeFixture } from "./helpers.js";

describe("runtime-import idempotency", () => {
  it("is a no-op on the second run after markers are removed", () => {
    withScratchDir("idempotency", (dir) => {
      const target = writeFixture(
        dir,
        "prompt.md",
        "before\n{{#runtime-import ./snippet.md}}\nafter\n",
      );
      writeFixture(dir, "snippet.md", "stable\n");

      const first = runImportSource(target);
      const afterFirst = readText(target);
      const second = runImportSource(target);

      expect(first.status).toBe(0);
      expect(second.status).toBe(0);
      expect(afterFirst).toBe("before\nstable\n\nafter\n");
      expect(readText(target)).toBe(afterFirst);
    });
  });
});
