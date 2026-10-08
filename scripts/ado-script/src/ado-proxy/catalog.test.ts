/**
 * Tests for the catalog helpers the enforcement path depends on.
 *
 * The catalog data itself is guarded by `catalog-drift.test.ts`; these cover
 * the lookup semantics layered over it, where a subtle mistake (a suffix match
 * on a host, say) would hand the injected bearer to the wrong destination.
 */
import { describe, expect, it } from "vitest";

import {
  API_VERSION_MAX,
  API_VERSION_MIN,
  CATALOG_SCHEMA_VERSION,
  DENIED_ROUTE_FAMILIES,
  isProtectedHost,
  OPERATIONS,
  operationsFor,
  PACKAGE_HOSTS,
  PACKAGE_PROTOCOLS,
  PACKAGE_REDIRECT_HOST_SUFFIXES,
  PACKAGE_RESPONSE_LIMIT,
  PROTECTED_HOSTS,
} from "./catalog.js";

describe("isProtectedHost", () => {
  it("matches the exact protected hosts, case-insensitively", () => {
    expect(isProtectedHost("dev.azure.com")).toBe(true);
    expect(isProtectedHost("DEV.AZURE.COM")).toBe(true);
    expect(isProtectedHost("  dev.azure.com  ")).toBe(true);
    expect(isProtectedHost("app.vssps.visualstudio.com")).toBe(true);
  });

  it("does not match look-alike hosts by suffix or prefix", () => {
    // A suffix match here would mean an attacker-controlled domain got the
    // injected bearer; a prefix match would leak it to a sibling service.
    expect(isProtectedHost("dev.azure.com.evil.test")).toBe(false);
    expect(isProtectedHost("notdev.azure.com")).toBe(false);
    expect(isProtectedHost("evil.test")).toBe(false);
  });

  it("canonicalizes trailing-dot FQDNs and CONNECT host:port targets", () => {
    // `dev.azure.com.` is an absolute FQDN for the *same* origin. Treating it
    // as unprotected would route it down the plain byte-tunnel path, skipping
    // TLS termination and catalog enforcement entirely.
    expect(isProtectedHost("dev.azure.com.")).toBe(true);
    expect(isProtectedHost("dev.azure.com..")).toBe(true);
    expect(isProtectedHost("DEV.AZURE.COM.")).toBe(true);
    // CONNECT targets always carry a port.
    expect(isProtectedHost("dev.azure.com:443")).toBe(true);
    expect(isProtectedHost("dev.azure.com.:443")).toBe(true);
    // Canonicalization must not create a match that isn't there.
    expect(isProtectedHost("dev.azure.com.evil.test.")).toBe(false);
    expect(isProtectedHost("")).toBe(false);
    expect(isProtectedHost(".")).toBe(false);
  });

  it("never protects IP literals", () => {
    // Raw-IP destinations bypass domain policy by definition; they must take
    // the unprotected path and are separately denied by Squid.
    expect(isProtectedHost("13.107.42.20")).toBe(false);
    expect(isProtectedHost("[::1]")).toBe(false);
    expect(isProtectedHost("[::1]:443")).toBe(false);
  });

  it("leaves package, artifact, and token hosts unprotected", () => {
    // These stay on the normal Squid path and must never be TLS-terminated or
    // receive the bearer — package restore has its own feed credentials.
    for (const host of [
      "pkgs.dev.azure.com",
      "artifacts.dev.azure.com",
      "vstoken.dev.azure.com",
      "vssps.dev.azure.com",
    ]) {
      expect(isProtectedHost(host)).toBe(false);
    }
  });
});

describe("operationsFor", () => {
  it("returns only operations in the enabled capability set", () => {
    const repos = operationsFor(["repos"]);
    expect(repos.length).toBeGreaterThan(0);
    expect(repos.every((operation) => operation.capability === "repos")).toBe(
      true,
    );
  });

  it("returns nothing when no capability is enabled", () => {
    expect(operationsFor([])).toHaveLength(0);
  });

  it("is additive across capabilities", () => {
    const discovery = operationsFor(["discovery"]).length;
    const repos = operationsFor(["repos"]).length;
    expect(operationsFor(["discovery", "repos"])).toHaveLength(
      discovery + repos,
    );
  });
});

describe("catalog surface", () => {
  it("exposes the schema version the policy document pins against", () => {
    expect(CATALOG_SCHEMA_VERSION).toBe("ado-aw/ado-proxy-catalog/v2");
  });

  it("catalogues only read-shaped methods", () => {
    for (const operation of OPERATIONS) {
      expect(["GET", "OPTIONS"]).toContain(operation.method);
    }
  });

  it("keeps credential-bearing families denied", () => {
    expect(DENIED_ROUTE_FAMILIES).toContain("/_apis/serviceendpoint");
    expect(DENIED_ROUTE_FAMILIES).toContain("/_git/");
  });

  it("exposes an ordered API-version window", () => {
    expect(API_VERSION_MIN[0]).toBeLessThanOrEqual(API_VERSION_MAX[0]);
  });
});

describe("package family surface", () => {
  it("keeps package hosts out of the always-protected REST set", () => {
    // Package hosts are intercepted only when the policy asks for it; being in
    // PROTECTED_HOSTS would intercept them for every workflow.
    for (const host of PACKAGE_HOSTS) {
      expect(PROTECTED_HOSTS).not.toContain(host);
      expect(isProtectedHost(host)).toBe(false);
    }
    expect(PACKAGE_HOSTS).toContain("pkgs.dev.azure.com");
  });

  it("catalogues only read methods for every package protocol", () => {
    for (const route of PACKAGE_PROTOCOLS) {
      expect(route.methods.every((method) => method === "GET" || method === "HEAD")).toBe(true);
      expect(route.path_prefixes.length).toBeGreaterThan(0);
      for (const prefix of route.path_prefixes) {
        expect(prefix).toBe(prefix.toLowerCase());
        expect(prefix.endsWith("/")).toBe(true);
      }
    }
  });

  it("allows an encoded slash only for npm", () => {
    const allowing = PACKAGE_PROTOCOLS.filter((route) => route.allow_encoded_slash);
    expect(allowing.map((route) => route.protocol)).toEqual(["npm"]);
  });

  it("anchors every redirect suffix at a label boundary", () => {
    for (const suffix of PACKAGE_REDIRECT_HOST_SUFFIXES) {
      expect(suffix.startsWith(".")).toBe(true);
    }
    expect(PACKAGE_RESPONSE_LIMIT).toBe(512 * 1024 * 1024);
  });
});
