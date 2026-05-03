import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { getWebApi, _resetCacheForTesting } from "./auth.js";

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
    process.env.ADO_COLLECTION_URI = "https://example/";
    delete process.env.SYSTEM_ACCESSTOKEN;
    await expect(getWebApi()).rejects.toThrow(/SYSTEM_ACCESSTOKEN/);
  });

  it("throws when ADO_COLLECTION_URI is missing", async () => {
    delete process.env.ADO_COLLECTION_URI;
    process.env.SYSTEM_ACCESSTOKEN = "tok";
    await expect(getWebApi()).rejects.toThrow(/ADO_COLLECTION_URI/);
  });

  it("caches the WebApi across calls", async () => {
    process.env.ADO_COLLECTION_URI = "https://example.visualstudio.com/";
    process.env.SYSTEM_ACCESSTOKEN = "tok";
    const a = await getWebApi();
    const b = await getWebApi();
    expect(a).toBe(b);
  });
});
