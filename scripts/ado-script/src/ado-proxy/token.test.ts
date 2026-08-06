import { describe, expect, it } from "vitest";

import { TokenError, TokenSource, bearerHeader } from "./token.js";

describe("TokenSource", () => {
  it("holds the bearer supplied at construction", () => {
    expect(new TokenSource("abc123").read()).toBe("abc123");
  });

  it("trims surrounding whitespace", () => {
    // The token arrives in a piped stream, so a trailing newline is expected.
    expect(new TokenSource("  abc123\n").read()).toBe("abc123");
  });

  it("rejects an empty or whitespace-only bearer at construction", () => {
    // Fail at startup rather than per request: forwarding unauthenticated
    // would make Azure DevOps answer with a sign-in page, which a client can
    // mistake for data.
    for (const value of ["", "   ", "\n\t "]) {
      expect(() => new TokenSource(value)).toThrow(TokenError);
    }
  });

  it("returns the same value on every read", () => {
    // No rotation by design: the token arrives once, on stdin. The compiler
    // bounds `timeout-minutes` so a run cannot outlive it.
    const source = new TokenSource("stable");
    expect(source.read()).toBe("stable");
    expect(source.read()).toBe("stable");
  });
});

describe("bearerHeader", () => {
  it("formats an AAD access token as a bearer", () => {
    expect(bearerHeader("abc")).toBe("Bearer abc");
  });
});
