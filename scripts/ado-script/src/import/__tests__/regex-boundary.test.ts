import { describe, expect, it } from "vitest";
import { readText, runImportSource, withScratchDir, writeFixture } from "./helpers.js";

describe("runtime-import regex boundaries", () => {
  it("expands markers with surrounding text and multiple matches in one file", () => {
    withScratchDir("regex-boundaries", (dir) => {
      const target = writeFixture(
        dir,
        "prompt.md",
        [
          "alpha {{#runtime-import ./one.md}} omega",
          "{{#runtime-import ./two.md}}",
          "pair {{#runtime-import ./three.md}} + {{#runtime-import ./four.md}} done",
          "",
        ].join("\n"),
      );
      writeFixture(dir, "one.md", "ONE");
      writeFixture(dir, "two.md", "TWO");
      writeFixture(dir, "three.md", "THREE");
      writeFixture(dir, "four.md", "FOUR");

      const result = runImportSource(target);

      expect(result.status).toBe(0);
      expect(readText(target)).toBe(
        ["alpha ONE omega", "TWO", "pair THREE + FOUR done", ""].join("\n"),
      );
    });
  });
});
