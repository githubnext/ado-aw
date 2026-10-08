/**
 * Parser tests for the piped interception material.
 *
 * These use synthetic PEM blocks rather than real `openssl` output: the parser
 * cares about *structure*, and shape-only fixtures keep the suite fast and free
 * of a toolchain dependency. Real material is exercised end to end in
 * `proxy.e2e.test.ts`.
 */
import {
  closeSync,
  mkdtempSync,
  openSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import {
  CaError,
  credentialProblem,
  MATERIAL_SCHEMA,
  parseCaMaterials,
  publishCaCertificate,
  readCaMaterials,
} from "./ca.js";
import { CATALOG_SCHEMA_VERSION } from "./catalog.js";
import type { ProxyPolicy } from "./config.js";

const KEY = "-----BEGIN PRIVATE KEY-----\nMIIfake\n-----END PRIVATE KEY-----\n";
const CERT = "-----BEGIN CERTIFICATE-----\nMIIfake\n-----END CERTIFICATE-----\n";
const TOKEN = "canary-bearer";

const b64 = (value: string): string => Buffer.from(value, "utf8").toString("base64");

function material(overrides: Record<string, unknown> = {}): string {
  return JSON.stringify({
    schema: MATERIAL_SCHEMA,
    ca_cert: b64(CERT),
    token: b64(TOKEN),
    leaves: { "dev.azure.com": { key: b64(KEY), cert: b64(CERT) } },
    ...overrides,
  });
}

describe("parseCaMaterials", () => {
  it("parses a well-formed document", () => {
    const materials = parseCaMaterials(material());
    expect(materials.caCertPem).toContain("BEGIN CERTIFICATE");
    expect(materials.token).toBe(TOKEN);
    expect(materials.leaves.get("dev.azure.com")?.key).toContain("BEGIN PRIVATE KEY");
  });

  it("lowercases hostnames so SNI lookup cannot miss on case", () => {
    const materials = parseCaMaterials(
      material({ leaves: { "DEV.Azure.COM": { key: b64(KEY), cert: b64(CERT) } } }),
    );
    expect(materials.leaves.has("dev.azure.com")).toBe(true);
  });

  it("rejects an empty stream", () => {
    // The likeliest real failure: the container was started without the pipe.
    expect(() => parseCaMaterials("")).toThrow(/no material on stdin/);
    expect(() => parseCaMaterials("   \n ")).toThrow(CaError);
  });

  it("rejects a truncated document loudly", () => {
    // The previous marker-based format could accept a partial stream; JSON
    // cannot, which is the main reason for the change.
    expect(() => parseCaMaterials(material().slice(0, 80))).toThrow(/not valid JSON/);
  });

  it("rejects a schema it does not implement", () => {
    // Producer and consumer are generated and shipped together; a mismatch
    // means one of them is stale, which must not silently under-enforce.
    expect(() => parseCaMaterials(material({ schema: "ado-aw/other/v9" }))).toThrow(
      /does not match/,
    );
    expect(() => parseCaMaterials(material({ schema: undefined }))).toThrow(/does not match/);
  });

  it("rejects a non-object document", () => {
    expect(() => parseCaMaterials("[]")).toThrow(/must be a JSON object/);
    expect(() => parseCaMaterials("null")).toThrow(/must be a JSON object/);
    expect(() => parseCaMaterials('"a string"')).toThrow(/must be a JSON object/);
  });

  it("rejects a document with no credential at all", () => {
    expect(() => parseCaMaterials(material({ token: undefined }))).toThrow(
      /neither token nor package_token/,
    );
  });

  it("rejects a present-but-empty credential rather than treating it as absent", () => {
    // An empty value means the host step meant to supply a credential and
    // failed; that must surface here, not as an unauthenticated forward.
    expect(() => parseCaMaterials(material({ token: "" }))).toThrow(/token/);
    expect(() => parseCaMaterials(material({ token: b64("   ") }))).toThrow(/token/);
    expect(() => parseCaMaterials(material({ package_token: "" }))).toThrow(/package_token/);
    expect(() => parseCaMaterials(material({ package_token: b64(" \n") }))).toThrow(
      /package_token/,
    );
    expect(() => parseCaMaterials(material({ package_token: "not!base64!" }))).toThrow(
      /package_token is not valid base64/,
    );
  });

  it("carries the REST and package credentials separately", () => {
    const both = parseCaMaterials(material({ package_token: b64("package-canary\n") }));
    expect(both.token).toBe(TOKEN);
    expect(both.packageToken).toBe("package-canary");

    const packagesOnly = parseCaMaterials(
      material({ token: undefined, package_token: b64("package-canary") }),
    );
    expect(packagesOnly.token).toBeUndefined();
    expect(packagesOnly.packageToken).toBe("package-canary");

    const restOnly = parseCaMaterials(material());
    expect(restOnly.packageToken).toBeUndefined();
  });

  it("rejects the previous material schema", () => {
    expect(MATERIAL_SCHEMA).toBe("ado-aw/ado-proxy-material/v2");
    expect(() =>
      parseCaMaterials(material({ schema: "ado-aw/ado-proxy-material/v1" })),
    ).toThrow(/does not match/);
  });

  it("rejects a document with no leaves", () => {
    // Without a leaf there is nothing to serve, so every intercepted request
    // would fail at handshake time with no clue as to why.
    expect(() => parseCaMaterials(material({ leaves: {} }))).toThrow(/no host leaves/);
    expect(() => parseCaMaterials(material({ leaves: undefined }))).toThrow(
      /must be a JSON object/,
    );
  });

  it("rejects a half-formed leaf rather than serving it", () => {
    expect(() =>
      parseCaMaterials(material({ leaves: { "dev.azure.com": { cert: b64(CERT) } } })),
    ).toThrow(/key must be a non-empty base64 string/);
    expect(() =>
      parseCaMaterials(material({ leaves: { "dev.azure.com": { key: b64(KEY) } } })),
    ).toThrow(/cert must be a non-empty base64 string/);
  });

  it("rejects a blob that is not really base64", () => {
    // Node's decoder silently drops invalid characters, so without the
    // round-trip check a corrupted blob would decode to wrong-but-plausible
    // bytes.
    expect(() => parseCaMaterials(material({ ca_cert: "not!valid!base64!" }))).toThrow(
      /not valid base64/,
    );
  });

  it("rejects base64 that decodes to something other than the expected PEM", () => {
    expect(() => parseCaMaterials(material({ ca_cert: b64("hello") }))).toThrow(
      /expected PEM block/,
    );
    expect(() =>
      parseCaMaterials(material({ leaves: { h: { key: b64(CERT), cert: b64(CERT) } } })),
    ).toThrow(/key does not contain the expected PEM block/);
  });

  it("cannot be tricked into fabricating a section from a value", () => {
    // The defect that motivated the format change: the old marker parser split
    // on "### " anywhere in the stream, so a value containing the marker text
    // produced a phantom host. JSON has no such ambiguity.
    const materials = parseCaMaterials(
      material({ token: b64('### HOST evil\n-----BEGIN PRIVATE KEY-----') }),
    );
    expect([...materials.leaves.keys()]).toEqual(["dev.azure.com"]);
    expect(materials.token).toContain("### HOST evil");
  });
});

describe("publishCaCertificate", () => {
  let directory: string;

  beforeEach(() => {
    directory = mkdtempSync(join(tmpdir(), "ado-proxy-ca-pub-"));
  });

  afterEach(() => {
    rmSync(directory, { recursive: true, force: true });
  });

  it("writes the public certificate", () => {
    expect(() => publishCaCertificate(join(directory, "ca.pem"), CERT)).not.toThrow();
  });

  it("makes the public certificate readable despite a restrictive umask", () => {
    const path = join(directory, "ca.pem");
    const previous = process.umask(0o077);
    try {
      publishCaCertificate(path, CERT);
    } finally {
      process.umask(previous);
    }

    // The non-root AWF agent and MCP child need read access. An observed runner
    // failure left this file at 0600 when writeFileSync's mode was filtered
    // through umask 077.
    const mode = statSync(path).mode & 0o777;
    if (process.platform === "win32") {
      // Windows exposes only the read-only attribute through chmod/stat; Node
      // commonly reports 0666 here. All three read bits are the portable
      // invariant, while the Linux runner must be exactly 0644.
      expect(mode & 0o444).toBe(0o444);
    } else {
      expect(mode).toBe(0o644);
    }
  });

  it("refuses to publish anything containing a private key", () => {
    // This path is mounted into the MCP container; a key reaching it would hand
    // out the ability to impersonate any protected host.
    expect(() => publishCaCertificate(join(directory, "ca.pem"), `${CERT}${KEY}`)).toThrow(
      /private key/,
    );
  });
});

describe("readCaMaterials", () => {
  let directory: string;

  beforeEach(() => {
    directory = mkdtempSync(join(tmpdir(), "ado-proxy-ca-read-"));
  });

  afterEach(() => {
    rmSync(directory, { recursive: true, force: true });
  });

  it("reads and parses from a descriptor", () => {
    const path = join(directory, "material.json");
    writeFileSync(path, material());
    const fd = openSync(path, "r");
    try {
      expect(readCaMaterials(fd).leaves.has("dev.azure.com")).toBe(true);
    } finally {
      closeSync(fd);
    }
  });

  it("reports an unreadable descriptor as a CaError", () => {
    expect(() => readCaMaterials(9999)).toThrow(CaError);
  });
});

describe("credentialProblem", () => {
  const POLICY: ProxyPolicy = {
    catalog_version: CATALOG_SCHEMA_VERSION,
    organization: "contoso",
    project: "Widgets",
    capabilities: ["core"],
    protected_hosts: ["dev.azure.com", "app.vssps.visualstudio.com"],
    allowed_resource_areas: [],
  };
  const PACKAGES = {
    hosts: ["pkgs.dev.azure.com"],
    feeds: [{ organization: "contoso", feed: "internal", protocols: ["npm" as const] }],
  };
  const leaves = (...hosts: string[]): Record<string, unknown> =>
    Object.fromEntries(hosts.map((host) => [host, { key: b64(KEY), cert: b64(CERT) }]));

  it("accepts a REST-only policy with a REST token", () => {
    expect(credentialProblem(POLICY, parseCaMaterials(material()))).toBeUndefined();
  });

  it("refuses REST capabilities without a REST token", () => {
    const ca = parseCaMaterials(material({ token: undefined, package_token: b64("p") }));
    expect(credentialProblem(POLICY, ca)).toMatch(/no REST token/);
  });

  it("refuses a packages section without a package credential", () => {
    const ca = parseCaMaterials(
      material({ leaves: leaves("dev.azure.com", "pkgs.dev.azure.com") }),
    );
    expect(credentialProblem({ ...POLICY, packages: PACKAGES }, ca)).toMatch(/no package_token/);
  });

  it("refuses a packages section without a leaf for the package host", () => {
    const ca = parseCaMaterials(material({ package_token: b64("p") }));
    expect(credentialProblem({ ...POLICY, packages: PACKAGES }, ca)).toMatch(
      /no leaf for pkgs\.dev\.azure\.com/,
    );
  });

  it("accepts a packages-only policy with only a package credential", () => {
    const ca = parseCaMaterials(
      material({
        token: undefined,
        package_token: b64("p"),
        leaves: leaves("dev.azure.com", "pkgs.dev.azure.com"),
      }),
    );
    expect(credentialProblem({ ...POLICY, capabilities: [], packages: PACKAGES }, ca)).toBeUndefined();
  });
});
