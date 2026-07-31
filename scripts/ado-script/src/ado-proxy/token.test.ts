import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { TokenError, TokenSource, bearerHeader } from "./token.js";

let directory: string;
let path: string;

beforeEach(() => {
  directory = mkdtempSync(join(tmpdir(), "ado-proxy-token-"));
  path = join(directory, "token");
});

afterEach(() => {
  rmSync(directory, { recursive: true, force: true });
});

describe("TokenSource", () => {
  it("reads and trims the token", () => {
    writeFileSync(path, "  abc123\n");
    expect(new TokenSource(path).read()).toBe("abc123");
  });

  it("throws when the file is missing", () => {
    // Never returns undefined: an unauthenticated forward would be answered by
    // Azure DevOps with a sign-in page the agent could mistake for data.
    expect(() => new TokenSource(path).read()).toThrow(TokenError);
  });

  it("throws when the file is empty or whitespace", () => {
    writeFileSync(path, "   \n");
    expect(() => new TokenSource(path).read()).toThrow(TokenError);
  });

  it("picks up a rotated token", () => {
    writeFileSync(path, "first");
    const source = new TokenSource(path);
    expect(source.read()).toBe("first");
    // Same length as "first" would leave size unchanged, so this also exercises
    // the mtime half of the cache key.
    writeFileSync(path, "secnd");
    expect(source.read()).toBe("secnd");
  });

  it("stops serving a cached token once the file disappears", () => {
    writeFileSync(path, "first");
    const source = new TokenSource(path);
    expect(source.read()).toBe("first");
    rmSync(path);
    expect(() => source.read()).toThrow(TokenError);
  });
});

describe("bearerHeader", () => {
  it("formats an AAD access token as a bearer", () => {
    expect(bearerHeader("abc")).toBe("Bearer abc");
  });
});
