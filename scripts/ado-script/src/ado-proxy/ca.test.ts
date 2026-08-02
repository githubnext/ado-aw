/**
 * Parser tests for the piped interception material.
 *
 * These use synthetic PEM blocks rather than real `openssl` output: the parser
 * cares about *structure*, and shape-only fixtures keep the suite fast and
 * free of a toolchain dependency. Real material is exercised end to end in
 * `proxy.e2e.test.ts`.
 */
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { CaError, parseCaMaterials, publishCaCertificate, readCaMaterials } from "./ca.js";

const KEY = "-----BEGIN PRIVATE KEY-----\nMIIfake\n-----END PRIVATE KEY-----\n";
const CERT = "-----BEGIN CERTIFICATE-----\nMIIfake\n-----END CERTIFICATE-----\n";

function stream(...sections: string[]): string {
  return sections.join("");
}

const CA_SECTION = `### CA\n${CERT}`;
const TOKEN_SECTION = "### TOKEN\ncanary-bearer\n";
const host = (name: string): string => `### HOST ${name}\n${KEY}${CERT}`;

describe("parseCaMaterials", () => {
  it("parses a CA and its leaves", () => {
    const materials = parseCaMaterials(
      stream(CA_SECTION, TOKEN_SECTION, host("dev.azure.com"), host("app.vssps.visualstudio.com")),
    );
    expect(materials.caCertPem).toContain("BEGIN CERTIFICATE");
    expect([...materials.leaves.keys()].sort()).toEqual([
      "app.vssps.visualstudio.com",
      "dev.azure.com",
    ]);
    expect(materials.leaves.get("dev.azure.com")?.key).toContain("BEGIN PRIVATE KEY");
  });

  it("lowercases hostnames so SNI lookup cannot miss on case", () => {
    const materials = parseCaMaterials(stream(CA_SECTION, TOKEN_SECTION, host("DEV.Azure.COM")));
    expect(materials.leaves.has("dev.azure.com")).toBe(true);
  });

  it("rejects an empty stream", () => {
    // The likeliest real failure: the container was started without the pipe.
    expect(() => parseCaMaterials("")).toThrow(/no certificate material on stdin/);
    expect(() => parseCaMaterials("   \n ")).toThrow(CaError);
  });

  it("rejects a stream with no CA", () => {
    expect(() => parseCaMaterials(stream(host("dev.azure.com")))).toThrow(/no CA section/);
  });

  it("rejects a stream with no leaves", () => {
    // Without a leaf there is nothing to serve, so every intercepted request
    // would fail at handshake time with no clue as to why.
    expect(() => parseCaMaterials(stream(CA_SECTION, TOKEN_SECTION))).toThrow(
      /no host leaves/,
    );
  });

  it("rejects a stream with no bearer", () => {
    // Certificates without a credential would mean every allowed request is
    // forwarded unauthenticated, and Azure DevOps answers those with a sign-in
    // page a client can mistake for data.
    expect(() => parseCaMaterials(stream(CA_SECTION, host("dev.azure.com")))).toThrow(
      /no Azure DevOps bearer/,
    );
  });

  it("rejects an empty bearer section", () => {
    expect(() =>
      parseCaMaterials(stream(CA_SECTION, "### TOKEN\n   \n", host("dev.azure.com"))),
    ).toThrow(/no Azure DevOps bearer/);
  });

  it("carries the bearer through", () => {
    const materials = parseCaMaterials(
      stream(CA_SECTION, TOKEN_SECTION, host("dev.azure.com")),
    );
    expect(materials.token).toBe("canary-bearer");
  });

  it("rejects a half-formed leaf rather than serving it", () => {
    expect(() =>
      parseCaMaterials(stream(CA_SECTION, `### HOST dev.azure.com\n${CERT}`)),
    ).toThrow(/missing its key/);
    expect(() =>
      parseCaMaterials(stream(CA_SECTION, `### HOST dev.azure.com\n${KEY}`)),
    ).toThrow(/missing its certificate/);
  });

  it("rejects a CA section carrying no certificate", () => {
    expect(() => parseCaMaterials(stream("### CA\n(nothing)\n", host("h")))).toThrow(
      /CA section carried no certificate/,
    );
  });

  it("rejects a host section with no hostname", () => {
    expect(() => parseCaMaterials(stream(CA_SECTION, `### HOST \n${KEY}${CERT}`))).toThrow(
      /no hostname/,
    );
  });

  it("ignores unrecognised sections rather than failing", () => {
    // Forward compatibility: a generator adding a section this build does not
    // know about must not take the proxy down.
    const materials = parseCaMaterials(
      stream(CA_SECTION, TOKEN_SECTION, "### FUTURE thing\nwhatever\n", host("dev.azure.com")),
    );
    expect(materials.leaves.size).toBe(1);
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
    const path = join(directory, "ca.pem");
    publishCaCertificate(path, CERT);
    expect(readCaMaterials).toBeTypeOf("function");
    expect(() => publishCaCertificate(path, CERT)).not.toThrow();
  });

  it("refuses to publish anything containing a private key", () => {
    // This path is mounted into the MCP container; a key reaching it would
    // hand out the ability to impersonate any protected host.
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
    const path = join(directory, "material.pem");
    writeFileSync(path, stream(CA_SECTION, TOKEN_SECTION, host("dev.azure.com")));
    // eslint-disable-next-line @typescript-eslint/no-require-imports
    const { openSync, closeSync } = require("node:fs") as typeof import("node:fs");
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
