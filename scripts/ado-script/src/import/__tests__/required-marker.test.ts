import { describe, expect, it } from "vitest";
import { readText, runImportSource, withScratchDir, writeFixture } from "./helpers.js";

describe("runtime-import required markers", () => {
  it("replaces a required marker with sibling snippet contents", () => {
    withScratchDir("required-marker", (dir) => {
      const target = writeFixture(
        dir,
        "prompt.md",
        "before\n{{#runtime-import ./snippet.md}}\nafter\n",
      );
      writeFixture(dir, "snippet.md", "hello from snippet\n");

      const result = runImportSource(target);

      expect(result.status).toBe(0);
      expect(result.stdout).toBe("");
      expect(result.stderr).toBe("");
      expect(readText(target)).toBe("before\nhello from snippet\n\nafter\n");
    });
  });
});
