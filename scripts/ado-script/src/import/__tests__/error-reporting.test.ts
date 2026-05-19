import { describe, expect, it } from "vitest";
import { readText, runImportSource, withScratchDir, writeFixture } from "./helpers.js";

describe("runtime-import error reporting", () => {
  it("reports every missing required marker, not just the first", () => {
    withScratchDir("multiple-missing", (dir) => {
      const original = [
        "head",
        "{{#runtime-import ./missing-one.md}}",
        "{{#runtime-import ./missing-two.md}}",
        "tail",
        "",
      ].join("\n");
      const target = writeFixture(dir, "prompt.md", original);

      const result = runImportSource(target);

      expect(result.status).toBe(1);
      expect(result.stderr).toBe("");
      expect(result.stdout).toBe(
        [
          "##vso[task.logissue type=error]runtime-import: file not found: ./missing-one.md",
          "##vso[task.logissue type=error]runtime-import: file not found: ./missing-two.md",
          "",
        ].join("\n"),
      );
      // Target file must NOT be overwritten on error.
      expect(readText(target)).toBe(original);
    });
  });

  it("strips characters that would break the ##vso command framing", () => {
    withScratchDir("vso-injection", (dir) => {
      // The path contains `]` which would close the ##vso bracket
      // prematurely, and the marker is the only one in the file so the
      // diagnostic line is deterministic.
      const target = writeFixture(
        dir,
        "prompt.md",
        "{{#runtime-import ./bad]type=warning]injected.md}}\n",
      );

      const result = runImportSource(target);

      expect(result.status).toBe(1);
      expect(result.stderr).toBe("");
      // `]` characters are stripped from the path in the error message.
      expect(result.stdout).toBe(
        "##vso[task.logissue type=error]runtime-import: file not found: ./badtype=warninginjected.md\n",
      );
    });
  });
});
