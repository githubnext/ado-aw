import { describe, expect, it } from "vitest";
import { readText, runImportSource, withScratchDir, writeFixture } from "./helpers.js";

describe("runtime-import path traversal", () => {
  it("rejects a required marker whose path contains a '..' segment", () => {
    withScratchDir("traversal-required", (dir) => {
      const original = "before\n{{#runtime-import ../escape.md}}\nafter\n";
      const target = writeFixture(dir, "prompt.md", original);

      const result = runImportSource(target);

      expect(result.status).toBe(1);
      expect(result.stderr).toBe("");
      expect(result.stdout).toBe(
        "##vso[task.logissue type=error]runtime-import: invalid path '../escape.md': '..' path components are not allowed\n",
      );
      // Target file must NOT be overwritten on error.
      expect(readText(target)).toBe(original);
    });
  });

  it("rejects '..' segments even when the marker is optional", () => {
    withScratchDir("traversal-optional", (dir) => {
      const original = "before\n{{#runtime-import? ../escape.md}}\nafter\n";
      const target = writeFixture(dir, "prompt.md", original);

      const result = runImportSource(target);

      // The traversal guard fires regardless of optional-marker form:
      // path traversal is structurally invalid, not a missing-file case.
      expect(result.status).toBe(1);
      expect(result.stdout).toContain("'..' path components are not allowed");
      expect(readText(target)).toBe(original);
    });
  });

  it("rejects backslash-style '..' segments on Windows-shaped paths", () => {
    withScratchDir("traversal-backslash", (dir) => {
      const target = writeFixture(
        dir,
        "prompt.md",
        "{{#runtime-import sub\\..\\..\\escape.md}}\n",
      );

      const result = runImportSource(target);

      expect(result.status).toBe(1);
      expect(result.stdout).toContain("'..' path components are not allowed");
    });
  });

  it("allows literal '..' inside a filename (not a segment)", () => {
    withScratchDir("traversal-literal", (dir) => {
      writeFixture(dir, "name..md", "DOUBLE_DOT_FILE");
      const target = writeFixture(
        dir,
        "prompt.md",
        "{{#runtime-import ./name..md}}\n",
      );

      const result = runImportSource(target);

      expect(result.status).toBe(0);
      expect(readText(target)).toBe("DOUBLE_DOT_FILE\n");
    });
  });
});
