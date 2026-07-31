/**
 * Configuration and policy-validation tests for the `ado-proxy` bundle.
 *
 * These cover the fail-closed startup contract: the proxy must refuse to serve
 * rather than start with a policy it cannot fully honour, because a running
 * proxy with a bad policy is an open tunnel to the protected hosts.
 */
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { beforeEach, describe, expect, it } from "vitest";

import { CATALOG_SCHEMA_VERSION } from "./catalog.js";
import { ConfigError, loadConfig, parsePolicy } from "./config.js";

const VALID_POLICY = {
  catalog_version: CATALOG_SCHEMA_VERSION,
  organization: "contoso",
  project: "Playground",
  project_id: "01234567-89ab-cdef-0123-456789abcdef",
  repository: "app",
  capabilities: ["discovery", "repos"],
  protected_hosts: ["dev.azure.com", "app.vssps.visualstudio.com"],
  allowed_resource_areas: [],
};

function policyJson(overrides: Record<string, unknown> = {}): string {
  return JSON.stringify({ ...VALID_POLICY, ...overrides });
}

/** Env vars the loader reads; cleared so host state cannot leak into a test. */
const PROXY_ENV_KEYS = [
  "ADO_PROXY_POLICY_FILE",
  "ADO_PROXY_TOKEN_FILE",
  "AWF_POLICY_PROXY_LISTEN_ADDRESS",
  "AWF_POLICY_PROXY_LISTEN_PORT",
  "AWF_POLICY_PROXY_UPSTREAM_PROXY",
  "AWF_POLICY_PROXY_PUBLIC_CA_PATH",
  "AWF_POLICY_PROXY_LOG_DIR",
];

beforeEach(() => {
  for (const key of PROXY_ENV_KEYS) delete process.env[key];
});

describe("parsePolicy", () => {
  it("accepts a well-formed policy", () => {
    const policy = parsePolicy(policyJson());
    expect(policy.organization).toBe("contoso");
    expect(policy.project).toBe("Playground");
    expect(policy.capabilities).toEqual(["discovery", "repos"]);
  });

  it("rejects a catalog_version the bundle does not implement", () => {
    // The central anti-divergence guarantee: a stale mounted policy must fail
    // closed rather than under-enforce against a newer catalog.
    expect(() =>
      parsePolicy(policyJson({ catalog_version: "ado-aw/ado-proxy-catalog/v0" })),
    ).toThrow(/does not match/);
  });

  it("rejects an unknown capability", () => {
    expect(() =>
      parsePolicy(policyJson({ capabilities: ["discovery", "everything"] })),
    ).toThrow(/unknown capability/);
  });

  it("rejects an empty protected-host set", () => {
    // With no protected hosts the proxy would tunnel everything unchecked.
    expect(() => parsePolicy(policyJson({ protected_hosts: [] }))).toThrow(
      /protected_hosts must not be empty/,
    );
  });

  it("rejects a protected-host set that omits a catalogued host", () => {
    // A catalogued host missing from the policy would take the byte-tunnel
    // path to Squid instead of being policed — the one bypass this proxy
    // exists to prevent.
    expect(() =>
      parsePolicy(policyJson({ protected_hosts: ["dev.azure.com"] })),
    ).toThrow(/omits the catalogued host app\.vssps\.visualstudio\.com/);
  });

  it("rejects an unknown key rather than ignoring it", () => {
    // An unrecognized key means the compiler emitted a constraint this bundle
    // does not implement; ignoring it would silently under-enforce.
    expect(() => parsePolicy(policyJson({ max_requests_per_minute: 10 }))).toThrow(
      /unknown key/,
    );
  });

  it.each([
    ["organization", { organization: "" }],
    ["project", { project: "" }],
  ])("rejects a missing %s scope", (_label, overrides) => {
    expect(() => parsePolicy(policyJson(overrides))).toThrow(ConfigError);
  });

  it("rejects malformed JSON and non-object documents", () => {
    expect(() => parsePolicy("{not json")).toThrow(/not valid JSON/);
    expect(() => parsePolicy("[]")).toThrow(/must be a JSON object/);
    expect(() => parsePolicy("null")).toThrow(/must be a JSON object/);
  });

  it("treats optional scope ids as absent rather than empty", () => {
    const policy = parsePolicy(
      policyJson({ repository: undefined, repository_id: undefined }),
    );
    expect(policy.repository).toBeUndefined();
    expect(policy.repository_id).toBeUndefined();
  });
});

describe("loadConfig", () => {
  function writePolicy(): string {
    const dir = mkdtempSync(join(tmpdir(), "ado-proxy-config-"));
    const path = join(dir, "policy.json");
    writeFileSync(path, policyJson());
    return path;
  }

  const baseArgs = (policyFile: string): string[] => [
    "--policy-file",
    policyFile,
    "--token-file",
    "/private/token",
    "--public-ca-file",
    "/ca/ca.pem",
    "--upstream-proxy",
    "http://squid-proxy:3128",
  ];

  it("resolves flags and applies defaults", () => {
    const config = loadConfig(baseArgs(writePolicy()));
    expect(config.listenAddress).toBe("0.0.0.0");
    expect(config.listenPort).toBe(11080);
    expect(config.upstreamProxy).toBe("http://squid-proxy:3128");
    expect(config.policy.organization).toBe("contoso");
  });

  it("accepts --flag=value form", () => {
    const policyFile = writePolicy();
    const config = loadConfig([
      `--policy-file=${policyFile}`,
      "--token-file=/private/token",
      "--public-ca-file=/ca/ca.pem",
      "--upstream-proxy=http://squid-proxy:3128",
      "--listen-port=12000",
    ]);
    expect(config.listenPort).toBe(12000);
  });

  it("falls back to the AWF environment contract", () => {
    const policyFile = writePolicy();
    process.env.ADO_PROXY_POLICY_FILE = policyFile;
    process.env.ADO_PROXY_TOKEN_FILE = "/private/token";
    process.env.AWF_POLICY_PROXY_PUBLIC_CA_PATH = "/ca/ca.pem";
    process.env.AWF_POLICY_PROXY_UPSTREAM_PROXY = "http://squid-proxy:3128";
    process.env.AWF_POLICY_PROXY_LISTEN_PORT = "13000";

    const config = loadConfig([]);
    expect(config.listenPort).toBe(13000);
    expect(config.tokenFile).toBe("/private/token");
  });

  it("requires an upstream proxy", () => {
    // Squid is the only route out; without it there is no egress path at all,
    // and silently defaulting would risk a direct-internet fallback.
    const policyFile = writePolicy();
    expect(() =>
      loadConfig([
        "--policy-file",
        policyFile,
        "--token-file",
        "/private/token",
        "--public-ca-file",
        "/ca/ca.pem",
      ]),
    ).toThrow(/--upstream-proxy/);
  });

  it("rejects an unusable listen port", () => {
    const policyFile = writePolicy();
    for (const port of ["0", "70000", "not-a-port"]) {
      expect(() =>
        loadConfig([...baseArgs(policyFile), "--listen-port", port]),
      ).toThrow(/listen-port/);
    }
  });

  it("reports an unreadable policy file", () => {
    expect(() =>
      loadConfig(baseArgs(join(tmpdir(), "ado-proxy-does-not-exist.json"))),
    ).toThrow(/cannot read policy file/);
  });

  it("never carries a credential in its resolved configuration", () => {
    // The bearer lives in a private file the trusted host task rotates; only
    // its *path* may appear in configuration.
    const config = loadConfig(baseArgs(writePolicy()));
    expect(JSON.stringify(config)).not.toContain("Bearer");
    expect(config.tokenFile).toBe("/private/token");
  });
});
