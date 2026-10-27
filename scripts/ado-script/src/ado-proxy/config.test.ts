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

  it("accepts well-formed additional scopes", () => {
    const policy = parsePolicy(
      policyJson({
        additional_scopes: [
          {
            organization: "fabrikam",
            projects: [
              {
                project: "Shared",
                project_id: "33333333-3333-3333-3333-333333333333",
                project_scoped: true,
                repositories: ["shared-api"],
              },
            ],
          },
        ],
      }),
    );

    expect(policy.additional_scopes).toEqual([
      {
        organization: "fabrikam",
        projects: [
          {
            project: "Shared",
            project_id: "33333333-3333-3333-3333-333333333333",
            project_scoped: true,
            repositories: ["shared-api"],
          },
        ],
      },
    ]);
  });

  it("rejects unknown keys at every additional-scope level", () => {
    expect(() =>
      parsePolicy(
        policyJson({
          additional_scopes: [
            {
              organization: "fabrikam",
              projects: [{ project: "Shared" }],
              all_projects: true,
            },
          ],
        }),
      ),
    ).toThrow(/additional_scopes\[0\].*unknown key/);

    expect(() =>
      parsePolicy(
        policyJson({
          additional_scopes: [
            {
              organization: "fabrikam",
              projects: [{ project: "Shared", all_repositories: true }],
            },
          ],
        }),
      ),
    ).toThrow(/projects\[0\].*unknown key/);
  });

  it("rejects an organization scope naming no projects", () => {
    expect(() =>
      parsePolicy(
        policyJson({
          additional_scopes: [{ organization: "fabrikam", projects: [] }],
        }),
      ),
    ).toThrow(/lists no projects/);
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

describe("parsePolicy packages section", () => {
  const FEED = {
    organization: "contoso",
    project: "Engineering",
    feed: "internal",
    view: "Release",
    protocols: ["npm", "nuget"],
  };
  const packagesJson = (
    overrides: Record<string, unknown> = {},
    feedOverrides: Record<string, unknown> = {},
  ): string =>
    policyJson({
      packages: {
        hosts: ["pkgs.dev.azure.com"],
        feeds: [{ ...FEED, ...feedOverrides }],
        ...overrides,
      },
    });

  it("is absent when the document carries no packages key", () => {
    expect(parsePolicy(policyJson()).packages).toBeUndefined();
  });

  it("accepts a well-formed section, with and without resolved ids", () => {
    const policy = parsePolicy(packagesJson());
    expect(policy.packages).toEqual({ hosts: ["pkgs.dev.azure.com"], feeds: [FEED] });

    const resolved = parsePolicy(
      packagesJson(
        {},
        {
          project_id: "11111111-1111-1111-1111-111111111111",
          feed_id: "22222222-2222-2222-2222-222222222222",
          view_id: "33333333-3333-3333-3333-333333333333",
        },
      ),
    );
    expect(resolved.packages?.feeds[0]?.feed_id).toBe("22222222-2222-2222-2222-222222222222");
  });

  it("accepts an organization-scoped feed with no view", () => {
    const policy = parsePolicy(
      packagesJson({}, { project: undefined, view: undefined, protocols: ["pypi"] }),
    );
    expect(policy.packages?.feeds[0]).toEqual({
      organization: "contoso",
      feed: "internal",
      protocols: ["pypi"],
    });
  });

  it("accepts a packages-only policy with no REST capability", () => {
    // Every REST operation is then denied by the authorizer; the policy itself
    // is legitimate.
    const policy = parsePolicy(
      JSON.stringify({
        ...VALID_POLICY,
        capabilities: [],
        packages: { hosts: ["pkgs.dev.azure.com"], feeds: [FEED] },
      }),
    );
    expect(policy.capabilities).toEqual([]);
    expect(policy.packages?.feeds).toHaveLength(1);
  });

  it("rejects unknown keys in the section and in a feed", () => {
    expect(() => parsePolicy(packagesJson({ allow_publish: true }))).toThrow(
      /policy\.packages has unknown key/,
    );
    expect(() => parsePolicy(packagesJson({}, { upstream: "allow" }))).toThrow(
      /feeds\[0\] has unknown key/,
    );
  });

  it("rejects a host set that omits or extends the catalogued package hosts", () => {
    // Omitting the host would leave package traffic unpoliced; an extra host
    // would be intercepted with no rules the authorizer understands.
    expect(() => parsePolicy(packagesJson({ hosts: [] }))).toThrow(/omits the catalogued/);
    expect(() =>
      parsePolicy(packagesJson({ hosts: ["pkgs.dev.azure.com", "evil.test"] })),
    ).toThrow(/not a catalogued package host/);
    expect(() => parsePolicy(packagesJson({ hosts: "pkgs.dev.azure.com" }))).toThrow(
      /hosts must be an array/,
    );
    expect(parsePolicy(packagesJson({ hosts: ["PKGS.dev.azure.com."] })).packages).toBeDefined();
  });

  it("rejects an empty feed list", () => {
    expect(() => parsePolicy(packagesJson({ feeds: [] }))).toThrow(/feeds must be a non-empty/);
    expect(() => parsePolicy(packagesJson({ feeds: undefined }))).toThrow(/feeds must be a non-empty/);
  });

  it("rejects empty, unknown, or duplicated protocols", () => {
    expect(() => parsePolicy(packagesJson({}, { protocols: [] }))).toThrow(/protocols must be/);
    expect(() => parsePolicy(packagesJson({}, { protocols: ["maven"] }))).toThrow(
      /unknown protocol: maven/,
    );
    expect(() => parsePolicy(packagesJson({}, { protocols: ["npm", "npm"] }))).toThrow(
      /more than once/,
    );
  });

  it.each(["organization", "feed"])("requires a non-empty %s", (key) => {
    expect(() => parsePolicy(packagesJson({}, { [key]: undefined }))).toThrow(
      new RegExp(`feeds\\[0\\]\\.${key} must be a non-empty string`),
    );
    expect(() => parsePolicy(packagesJson({}, { [key]: " " }))).toThrow(ConfigError);
  });

  it.each(["project", "view"])("rejects an empty optional %s", (key) => {
    expect(() => parsePolicy(packagesJson({}, { [key]: "" }))).toThrow(
      new RegExp(`${key} must be a non-empty string when present`),
    );
  });

  it.each(["project_id", "feed_id", "view_id"])("rejects a non-canonical %s", (key) => {
    for (const value of [
      "not-a-guid",
      "{11111111-1111-1111-1111-111111111111}",
      "11111111111111111111111111111111",
      "11111111-1111-1111-1111-11111111111g",
    ]) {
      expect(() => parsePolicy(packagesJson({}, { [key]: value }))).toThrow(/canonical GUID/);
    }
  });

  it("rejects an id without the name it identifies", () => {
    expect(() =>
      parsePolicy(
        packagesJson({}, { project: undefined, project_id: "11111111-1111-1111-1111-111111111111" }),
      ),
    ).toThrow(/project_id is set but project is not/);
    expect(() =>
      parsePolicy(
        packagesJson({}, { view: undefined, view_id: "11111111-1111-1111-1111-111111111111" }),
      ),
    ).toThrow(/view_id is set but view is not/);
  });

  it("rejects a non-object section or feed", () => {
    expect(() => parsePolicy(policyJson({ packages: [] }))).toThrow(/must be a JSON object/);
    expect(() =>
      parsePolicy(policyJson({ packages: { hosts: ["pkgs.dev.azure.com"], feeds: ["internal"] } })),
    ).toThrow(/feeds\[0\] must be a JSON object/);
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
      "--public-ca-file=/ca/ca.pem",
      "--upstream-proxy=http://squid-proxy:3128",
      "--listen-port=12000",
    ]);
    expect(config.listenPort).toBe(12000);
  });

  it("falls back to the AWF environment contract", () => {
    const policyFile = writePolicy();
    process.env.ADO_PROXY_POLICY_FILE = policyFile;
    process.env.AWF_POLICY_PROXY_PUBLIC_CA_PATH = "/ca/ca.pem";
    process.env.AWF_POLICY_PROXY_UPSTREAM_PROXY = "http://squid-proxy:3128";
    process.env.AWF_POLICY_PROXY_LISTEN_PORT = "13000";

    const config = loadConfig([]);
    expect(config.listenPort).toBe(13000);
  });

  it("requires an upstream proxy", () => {
    // Squid is the only route out; without it there is no egress path at all,
    // and silently defaulting would risk a direct-internet fallback.
    const policyFile = writePolicy();
    expect(() =>
      loadConfig([
        "--policy-file",
        policyFile,
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
  });
});
