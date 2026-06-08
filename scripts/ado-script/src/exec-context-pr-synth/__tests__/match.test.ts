import { describe, expect, it } from "vitest";

import {
  matchesIncludeExclude,
  normaliseBranchRef,
  normalisePath,
  pathMatchesIncludeExclude,
} from "../match.js";

describe("normaliseBranchRef", () => {
  it("strips refs/heads/ prefix", () => {
    expect(normaliseBranchRef("refs/heads/main")).toBe("main");
    expect(normaliseBranchRef("refs/heads/feature/x")).toBe("feature/x");
  });
  it("leaves other refs alone", () => {
    expect(normaliseBranchRef("main")).toBe("main");
    expect(normaliseBranchRef("refs/tags/v1")).toBe("refs/tags/v1");
  });
});

describe("normalisePath", () => {
  it("strips a leading slash", () => {
    expect(normalisePath("/src/foo.rs")).toBe("src/foo.rs");
  });
  it("leaves paths without a leading slash alone", () => {
    expect(normalisePath("src/foo.rs")).toBe("src/foo.rs");
  });
});

describe("matchesIncludeExclude (branches)", () => {
  it("matches when include list is empty (include-all)", () => {
    expect(matchesIncludeExclude("main", [], [])).toBe(true);
    expect(matchesIncludeExclude("refs/heads/main", [], [])).toBe(true);
  });

  it("matches exact branch names with refs/heads/ normalisation on BOTH sides", () => {
    expect(matchesIncludeExclude("refs/heads/main", ["main"], [])).toBe(true);
    expect(matchesIncludeExclude("main", ["refs/heads/main"], [])).toBe(true);
  });

  it("rejects branches that are not in the include list", () => {
    expect(matchesIncludeExclude("refs/heads/feature/x", ["main"], [])).toBe(false);
  });

  it("supports * glob inside one segment", () => {
    expect(matchesIncludeExclude("refs/heads/release/1.0", ["release/*"], [])).toBe(true);
  });

  it("supports ** glob across segments", () => {
    expect(matchesIncludeExclude("refs/heads/feature/x/y", ["feature/**"], [])).toBe(true);
  });

  it("exclude wins over include", () => {
    expect(matchesIncludeExclude("refs/heads/feature/x", ["**"], ["feature/*"])).toBe(false);
  });

  it("returns true when include matches and exclude does not", () => {
    expect(matchesIncludeExclude("refs/heads/main", ["main", "release/*"], ["dev/*"])).toBe(
      true,
    );
  });
});

describe("pathMatchesIncludeExclude (paths)", () => {
  it("matches when include list is empty (include-all)", () => {
    expect(pathMatchesIncludeExclude("/src/foo.rs", [], [])).toBe(true);
  });

  it("handles leading-slash normalisation on both sides", () => {
    expect(pathMatchesIncludeExclude("/src/foo.rs", ["/src/**"], [])).toBe(true);
    expect(pathMatchesIncludeExclude("src/foo.rs", ["src/**"], [])).toBe(true);
  });

  it("rejects paths that fall outside the include glob", () => {
    expect(pathMatchesIncludeExclude("/docs/x.md", ["src/**"], [])).toBe(false);
  });

  it("exclude wins over include", () => {
    expect(pathMatchesIncludeExclude("/tests/x.rs", ["**"], ["tests/**"])).toBe(false);
  });
});
