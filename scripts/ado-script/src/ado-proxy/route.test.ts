import { describe, expect, it } from "vitest";

import { DENIED_ROUTE_FAMILIES } from "./catalog.js";
import { matchRoute, matchesDeniedFamily, normalizeTarget, NormalizeError } from "./route.js";

describe("normalizeTarget", () => {
  it("splits a plain path into decoded segments", () => {
    const target = normalizeTarget("/myorg/_apis/projects/MyProject");
    expect(target.segments).toEqual(["myorg", "_apis", "projects", "MyProject"]);
    expect(target.query).toEqual([]);
  });

  it("decodes a segment exactly once", () => {
    expect(normalizeTarget("/org/_apis/projects/My%20Project").segments).toEqual([
      "org",
      "_apis",
      "projects",
      "My Project",
    ]);
  });

  it("tolerates a single trailing slash", () => {
    expect(normalizeTarget("/myorg/_apis/").segments).toEqual(["myorg", "_apis"]);
  });

  it("parses the query preserving order and duplicates", () => {
    // Collapsing duplicates would hide exactly the conflict the api-version
    // check needs to see.
    expect(normalizeTarget("/o/_apis?a=1&a=2&b=").query).toEqual([
      ["a", "1"],
      ["a", "2"],
      ["b", ""],
    ]);
  });

  it("rejects an encoded path separator", () => {
    // `%2f` would let one segment masquerade as several, so a route the policy
    // believes is bounded could address something else upstream.
    expect(() => normalizeTarget("/org/_apis/projects/a%2f..%2fb")).toThrow(
      NormalizeError,
    );
  });

  it("rejects double encoding", () => {
    // `%252e` decodes to `%2e` here but to `.` upstream — the two would
    // disagree about what path was authorized.
    expect(() => normalizeTarget("/org/_apis/%252e%252e")).toThrow(NormalizeError);
  });

  it("rejects traversal, empty segments, and non-origin-form targets", () => {
    expect(() => normalizeTarget("/org/../admin")).toThrow(NormalizeError);
    expect(() => normalizeTarget("/org//admin")).toThrow(NormalizeError);
    expect(() => normalizeTarget("https://dev.azure.com/org")).toThrow(NormalizeError);
    expect(() => normalizeTarget("/org/_apis#frag")).toThrow(NormalizeError);
  });

  it("rejects control characters in a segment", () => {
    expect(() => normalizeTarget("/org/_apis/pro%00ject")).toThrow(NormalizeError);
  });
});

describe("matchRoute", () => {
  it("matches literals case-insensitively and captures placeholders", () => {
    const params = matchRoute("/{org}/_apis/projects/{project}", [
      "myorg",
      "_APIS",
      "Projects",
      "MyProject",
    ]);
    expect(params).toEqual({ org: "myorg", project: "MyProject" });
  });

  it("requires an exact segment count", () => {
    expect(matchRoute("/{org}/_apis", ["myorg"])).toBeUndefined();
    expect(matchRoute("/{org}/_apis", ["myorg", "_apis", "extra"])).toBeUndefined();
  });

  it("enforces the shape of numeric id placeholders", () => {
    const route = "/{org}/_apis/wit/workitems/{id}";
    expect(matchRoute(route, ["o", "_apis", "wit", "workitems", "42"])).toEqual({
      org: "o",
      id: "42",
    });
    // A non-numeric id would smuggle a sub-resource or filter into a route the
    // catalog treats as fully bounded.
    for (const bad of ["42;x", "0", "abc", "42?x", "-1"]) {
      expect(matchRoute(route, ["o", "_apis", "wit", "workitems", bad])).toBeUndefined();
    }
  });

  it("enforces the shape of GUID and commit placeholders", () => {
    expect(
      matchRoute("/_apis/resourceareas/{areaId}", ["_apis", "resourceareas", "not-a-guid"]),
    ).toBeUndefined();
    expect(
      matchRoute("/_apis/resourceareas/{areaId}", [
        "_apis",
        "resourceareas",
        "79134c72-4a58-4b42-976c-04e7115f32bf",
      ]),
    ).toEqual({ areaId: "79134c72-4a58-4b42-976c-04e7115f32bf" });
  });

  it("does not constrain scope placeholders by shape", () => {
    // `org`, `project`, and `repository` are checked against the pinned policy
    // values instead, which is strictly stronger than a regex.
    expect(matchRoute("/{org}/_apis", ["Contoso Org", "_apis"])).toEqual({
      org: "Contoso Org",
    });
  });
});

describe("matchesDeniedFamily", () => {
  it("finds a denied family anywhere in the path", () => {
    expect(
      matchesDeniedFamily(
        ["org", "_apis", "serviceendpoint", "endpoints"],
        ["/_apis/serviceendpoint"],
      ),
    ).toBe("/_apis/serviceendpoint");
  });

  it("matches case-insensitively", () => {
    expect(matchesDeniedFamily(["org", "project", "_GIT", "repo"], ["/_git/"])).toBe(
      "/_git/",
    );
  });

  it("matches families that contain placeholders", () => {
    // A substring test cannot match these: the literal text
    // `{buildId}` never appears in a real path, so the denial would be
    // silently inert exactly where defence-in-depth matters most.
    for (const [path, family] of [
      [
        ["org", "proj", "_apis", "build", "builds", "42", "oauthtoken"],
        "/_apis/build/builds/{buildId}/oauthtoken",
      ],
      [
        ["org", "proj", "_apis", "build", "builds", "42", "artifacts"],
        "/_apis/build/builds/{buildId}/artifacts",
      ],
      [
        ["org", "proj", "_apis", "git", "repositories", "repo", "blobs"],
        "/_apis/git/repositories/{repository}/blobs",
      ],
    ] as [string[], string][]) {
      expect(matchesDeniedFamily(path, [family])).toBe(family);
    }
  });

  it("matches a family that pins a query parameter", () => {
    const family = "/_apis/wit/workitems?ids=";
    expect(
      matchesDeniedFamily(["org", "_apis", "wit", "workitems"], [family], [
        ["ids", "1,2,3"],
      ]),
    ).toBe(family);
    // Without the pinned parameter the family does not apply, so the ordinary
    // single-work-item route stays reachable.
    expect(
      matchesDeniedFamily(["org", "_apis", "wit", "workitems"], [family], []),
    ).toBeUndefined();
  });

  it("requires the family segments to be contiguous", () => {
    expect(
      matchesDeniedFamily(
        ["org", "_apis", "build", "builds", "42", "extra", "artifacts"],
        ["/_apis/build/builds/{buildId}/artifacts"],
      ),
    ).toBeUndefined();
  });

  it("does not match a segment by prefix", () => {
    // `serviceendpointproxy` is a different route; matching it here would be a
    // false denial, and matching `_gitignore` for `/_git/` would be worse.
    expect(
      matchesDeniedFamily(["org", "_apis", "serviceendpointproxy"], [
        "/_apis/serviceendpoint",
      ]),
    ).toBeUndefined();
  });

  it("returns undefined for an unrelated path", () => {
    expect(
      matchesDeniedFamily(["org", "_apis", "projects"], ["/_apis/serviceendpoint"]),
    ).toBeUndefined();
  });

  it("keeps every catalogued family expressible", () => {
    // Guards the inverse failure: a family the matcher can never match is a
    // denial the catalog author believes exists but nothing enforces.
    for (const family of DENIED_ROUTE_FAMILIES) {
      const segments = family
        .split("?")[0]
        ?.split("/")
        .filter((part) => part !== "")
        .map((part) => (part.startsWith("{") ? "placeholder" : part)) as string[];
      const query = family.includes("?")
        ? ([[family.split("?")[1]?.split("=")[0] ?? "", "x"]] as [string, string][])
        : [];
      expect(
        matchesDeniedFamily(segments, [family], query),
        `family ${family} is unmatchable`,
      ).toBe(family);
    }
  });
});
