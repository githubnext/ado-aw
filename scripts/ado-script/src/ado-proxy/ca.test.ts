/**
 * Parser tests for the piped interception material.
 *
 * These use synthetic PEM blocks rather than real `openssl` output: the parser
 * cares about *structure*, and shape-only fixtures keep the suite fast and free
 * of a toolchain dependency. Real material is exercised end to end in
 * `proxy.e2e.test.ts`.
 */
import { mkdtempSync, rmSync, writeFileSync, openSync, closeSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import {
  CaError,
  MATERIAL_SCHEMA,
  parseCaMaterials,
  publishCaCertificate,
  readCaMaterials,
} from "./ca.js";

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

  it("rejects a missing or empty bearer", () => {
    expect(() => parseCaMaterials(material({ token: undefined }))).toThrow(/token/);
    expect(() => parseCaMaterials(material({ token: "" }))).toThrow(/token/);
    expect(() => parseCaMaterials(material({ token: b64("   ") }))).toThrow(/token/);
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
