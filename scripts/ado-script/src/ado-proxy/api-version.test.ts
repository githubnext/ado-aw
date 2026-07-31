import { describe, expect, it } from "vitest";

import {
  API_VERSION_ABSENT,
  ApiVersionError,
  apiVersionFromAccept,
  parseApiVersion,
  resolveApiVersion,
} from "./api-version.js";

const RANGE = "5.0..=7.2; preview allowed";

describe("parseApiVersion", () => {
  it("parses plain and preview versions", () => {
    expect(parseApiVersion("7.1")).toMatchObject({ major: 7, minor: 1, preview: false });
    expect(parseApiVersion("7.1-preview")).toMatchObject({ preview: true });
    expect(parseApiVersion("7.1-preview.2")).toMatchObject({ preview: true });
  });

  it("rejects anything else", () => {
    for (const bad of ["7", "7.1.2", "v7.1", "7.1-alpha", ""]) {
      expect(() => parseApiVersion(bad)).toThrow(ApiVersionError);
    }
  });
});

describe("apiVersionFromAccept", () => {
  it("reads the parameter from a media type", () => {
    expect(
      apiVersionFromAccept("application/json;api-version=7.1;excludeUrls=true"),
    ).toBe("7.1");
  });

  it("is case-insensitive about the parameter name", () => {
    expect(apiVersionFromAccept("application/json;API-Version=6.0")).toBe("6.0");
  });

  it("returns undefined when absent", () => {
    expect(apiVersionFromAccept("application/json")).toBeUndefined();
    expect(apiVersionFromAccept(undefined)).toBeUndefined();
  });

  it("rejects conflicting versions across media types", () => {
    // Otherwise the policy validates one version while the upstream honours
    // another.
    expect(() =>
      apiVersionFromAccept("application/json;api-version=7.1, text/plain;api-version=1.0"),
    ).toThrow(ApiVersionError);
  });
});

describe("resolveApiVersion", () => {
  it("accepts a version supplied only in the query", () => {
    expect(resolveApiVersion(RANGE, [["api-version", "7.1"]], undefined)?.raw).toBe("7.1");
  });

  it("accepts a version supplied only in Accept", () => {
    expect(
      resolveApiVersion(RANGE, [], "application/json;api-version=6.0")?.raw,
    ).toBe("6.0");
  });

  it("accepts matching versions in both places", () => {
    expect(
      resolveApiVersion(RANGE, [["api-version", "7.0"]], "application/json;api-version=7.0")
        ?.raw,
    ).toBe("7.0");
  });

  it("rejects disagreement between the query and Accept", () => {
    expect(() =>
      resolveApiVersion(
        RANGE,
        [["api-version", "7.1"]],
        "application/json;api-version=3.0",
      ),
    ).toThrow(ApiVersionError);
  });

  it("rejects duplicate conflicting query parameters", () => {
    expect(() =>
      resolveApiVersion(RANGE, [
        ["api-version", "7.1"],
        ["api-version", "1.0"],
      ], undefined),
    ).toThrow(ApiVersionError);
  });

  it("requires a version on versioned operations", () => {
    expect(() => resolveApiVersion(RANGE, [], undefined)).toThrow(ApiVersionError);
  });

  it("rejects versions outside the catalog window", () => {
    // Old preview surfaces routinely expose fields and routes the catalog was
    // never written against.
    expect(() => resolveApiVersion(RANGE, [["api-version", "1.0"]], undefined)).toThrow(
      ApiVersionError,
    );
    expect(() => resolveApiVersion(RANGE, [["api-version", "9.0"]], undefined)).toThrow(
      ApiVersionError,
    );
  });

  it("requires absence on discovery OPTIONS operations", () => {
    expect(resolveApiVersion(API_VERSION_ABSENT, [], undefined)).toBeUndefined();
    expect(() =>
      resolveApiVersion(API_VERSION_ABSENT, [["api-version", "7.1"]], undefined),
    ).toThrow(ApiVersionError);
    expect(() =>
      resolveApiVersion(API_VERSION_ABSENT, [], "application/json;api-version=7.1"),
    ).toThrow(ApiVersionError);
  });
});
