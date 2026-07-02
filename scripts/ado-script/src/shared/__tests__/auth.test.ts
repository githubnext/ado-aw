import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { getWebApi, _resetCacheForTesting } from "../auth.js";

describe("getWebApi", () => {
  const originalEnv = { ...process.env };

  beforeEach(() => {
    _resetCacheForTesting();
    process.env = { ...originalEnv };
    // Suppress vso-logger output during tests
    vi.spyOn(process.stdout, "write").mockImplementation(() => true);
  });

  afterEach(() => {
    process.env = originalEnv;
    vi.restoreAllMocks();
  });

  it("throws when SYSTEM_ACCESSTOKEN is missing", async () => {
    process.env.SYSTEM_COLLECTIONURI = "https://example/";
    delete process.env.SYSTEM_ACCESSTOKEN;
    await expect(getWebApi()).rejects.toThrow(/SYSTEM_ACCESSTOKEN/);
  });

  it("throws when both collection URI vars are missing", async () => {
    delete process.env.SYSTEM_COLLECTIONURI;
    delete process.env.SYSTEM_TEAMFOUNDATIONCOLLECTIONURI;
    process.env.SYSTEM_ACCESSTOKEN = "tok";
    await expect(getWebApi()).rejects.toThrow(/SYSTEM_COLLECTIONURI/);
  });

  it("uses SYSTEM_COLLECTIONURI when present", async () => {
    process.env.SYSTEM_COLLECTIONURI = "https://example.visualstudio.com/";
    delete process.env.SYSTEM_TEAMFOUNDATIONCOLLECTIONURI;
    process.env.SYSTEM_ACCESSTOKEN = "tok";
    await expect(getWebApi()).resolves.toBeDefined();
  });

  it("falls back to SYSTEM_TEAMFOUNDATIONCOLLECTIONURI when SYSTEM_COLLECTIONURI is unset", async () => {
    delete process.env.SYSTEM_COLLECTIONURI;
    process.env.SYSTEM_TEAMFOUNDATIONCOLLECTIONURI =
      "https://example.visualstudio.com/";
    process.env.SYSTEM_ACCESSTOKEN = "tok";
    await expect(getWebApi()).resolves.toBeDefined();
  });

  it("caches the WebApi across calls", async () => {
    process.env.SYSTEM_COLLECTIONURI = "https://example.visualstudio.com/";
    process.env.SYSTEM_ACCESSTOKEN = "tok";
    const a = await getWebApi();
    const b = await getWebApi();
    expect(a).toBe(b);
  });
});
