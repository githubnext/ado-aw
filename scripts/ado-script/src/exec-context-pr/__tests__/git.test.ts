import { describe, expect, it } from "vitest";

import { bearerEnv } from "../git.js";

describe("bearerEnv", () => {
  it("returns the GIT_CONFIG_* triple when a token is present", () => {
    const env = bearerEnv("xyz-token");
    expect(env).toEqual({
      GIT_CONFIG_COUNT: "1",
      GIT_CONFIG_KEY_0: "http.extraheader",
      GIT_CONFIG_VALUE_0: "Authorization: bearer xyz-token",
    });
  });

  it("returns an empty object when token is undefined", () => {
    expect(bearerEnv(undefined)).toEqual({});
  });

  it("returns an empty object when token is empty string", () => {
    expect(bearerEnv("")).toEqual({});
  });

  it("places the token only in GIT_CONFIG_VALUE_0 (never in argv)", () => {
    const env = bearerEnv("secret");
    // The token must NOT appear as a value for any other key — sanity
    // check that bearerEnv hasn't been refactored to leak it elsewhere.
    expect(env.GIT_CONFIG_COUNT).toBe("1");
    expect(env.GIT_CONFIG_KEY_0).toBe("http.extraheader");
    expect(env.GIT_CONFIG_VALUE_0).toContain("secret");
    expect(Object.keys(env)).toHaveLength(3);
  });
});
