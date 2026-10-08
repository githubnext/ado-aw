/**
 * Interception certificates and the Azure DevOps bearer, supplied on stdin.
 *
 * The engine does **not** mint its own certificates. A host pipeline step runs
 * `openssl` — already an unconditional dependency of every compiled pipeline,
 * which mints the MCPG API key with `openssl rand` — and pipes the CA, the
 * per-host leaves, and the bearer straight into `docker run -i`. Two
 * consequences:
 *
 *   - **No bearer touches a filesystem, and no private key touches runner
 *     `/tmp`.** The host generates keys under `$(Agent.TempDirectory)`,
 *     streams them with the bearer through a container-local FIFO, and shreds
 *     them immediately after handover. AWF exposes runner `/tmp` inside the
 *     agent chroot, so using that path would make private material readable by
 *     the agent. The FIFO itself stores no bytes.
 *   - **The engine needs no `openssl`,** so it runs on `node:20-slim` (which
 *     has none) rather than the full `node:20`. That is already the image the
 *     Azure DevOps MCP uses, so it adds nothing to mirror.
 *
 * The protected host set is compiler-known, so every leaf is generated ahead of
 * time. Nothing here has to *issue* a certificate — fortunate, since Node can
 * parse X.509 but not issue it.
 *
 * ## Wire format
 *
 * A single JSON document, mirroring how MCPG already receives its config
 * (`echo "$MCPG_CONFIG" | docker run -i …`):
 *
 * ```json
 * {
 *   "schema": "ado-aw/ado-proxy-material/v2",
 *   "ca_cert": "<base64 PEM>",
 *   "token": "<base64>",
 *   "package_token": "<base64>",
 *   "leaves": { "dev.azure.com": { "key": "<base64 PEM>", "cert": "<base64 PEM>" } }
 * }
 * ```
 *
 * `token` is the REST bearer and `package_token` the Azure Artifacts credential.
 * Each is optional — a packages-only workflow carries no REST bearer, a
 * REST-only workflow no package credential — but at least one must be present,
 * and `index.ts` cross-checks them against the policy so a configured family is
 * never started without its credential. They are separate fields, rather than
 * one shared token, so each family's upstream only ever receives its own
 * credential.
 *
 * Blobs are base64 so the generating shell never has to escape newlines, and so
 * a corrupted blob fails at decode rather than yielding a subtly wrong
 * certificate. `JSON.parse` supplies the structural validation: a truncated
 * stream fails loudly, no value can fabricate a section, and `schema` fails
 * closed if producer and consumer ever diverge.
 *
 * An earlier revision used an ad-hoc `### MARKER` format. It was replaced
 * because marker matching was not anchored to line starts — a value containing
 * the marker text could fabricate a section — and duplicate sections resolved
 * silently to the last occurrence.
 */
import { chmodSync, readFileSync, writeFileSync } from "node:fs";

import { canonicalizeHost } from "./catalog.js";
import type { ProxyPolicy } from "./config.js";

export class CaError extends Error {}

/** Wire-format version, checked on parse so a mismatch fails closed. */
export const MATERIAL_SCHEMA = "ado-aw/ado-proxy-material/v2";

/** A leaf certificate and its key, for one protected host. */
export interface Leaf {
  readonly key: string;
  readonly cert: string;
}

/** Parsed interception material. */
export interface CaMaterials {
  /** PEM of the CA certificate. Safe to publish. */
  readonly caCertPem: string;
  /** Leaf key/cert per host, keyed by lowercase hostname. */
  readonly leaves: ReadonlyMap<string, Leaf>;
  /**
   * The Azure DevOps REST bearer, when the workflow enables REST capabilities.
   *
   * Carried in the same document as the certificates because it has the same
   * custody requirement: it must reach this process without touching a path
   * the agent can read.
   */
  readonly token?: string;
  /**
   * The Azure Artifacts credential, when the policy carries a `packages`
   * section. Same custody as {@link token}; never sent to a REST host.
   */
  readonly packageToken?: string;
}

const PRIVATE_KEY =
  /-----BEGIN (?:RSA |EC )?PRIVATE KEY-----[\s\S]*?-----END (?:RSA |EC )?PRIVATE KEY-----/;
const CERTIFICATE = /-----BEGIN CERTIFICATE-----[\s\S]*?-----END CERTIFICATE-----/;

function asRecord(value: unknown, label: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new CaError(`${label} must be a JSON object`);
  }
  return value as Record<string, unknown>;
}

/** Strip base64 padding so a round-trip comparison is not defeated by it. */
function withoutPadding(value: string): string {
  return value.replace(/=+$/, "");
}

/**
 * Decode one base64 field.
 *
 * The encoding is verified by re-encoding rather than trusted: Node's decoder
 * is lenient and silently drops invalid characters, so a corrupted blob would
 * otherwise decode to plausible-looking but wrong bytes.
 */
function decodeBase64(source: Record<string, unknown>, key: string, label: string): string {
  const value = source[key];
  if (typeof value !== "string" || value.trim() === "") {
    throw new CaError(`${label} must be a non-empty base64 string`);
  }
  const normalized = value.replace(/\s+/g, "");
  const decoded = Buffer.from(normalized, "base64");
  if (withoutPadding(decoded.toString("base64")) !== withoutPadding(normalized)) {
    throw new CaError(`${label} is not valid base64`);
  }
  const text = decoded.toString("utf8");
  if (text.trim() === "") {
    throw new CaError(`${label} decoded to nothing`);
  }
  return text;
}

function requirePem(text: string, pattern: RegExp, label: string): string {
  const match = pattern.exec(text)?.[0];
  if (match === undefined) {
    throw new CaError(`${label} does not contain the expected PEM block`);
  }
  return match;
}

/**
 * Parse the material document.
 *
 * Fails closed on anything incomplete or unrecognised: a wrong schema, a
 * missing CA, a host without both a key and a certificate, no hosts at all, a
 * present-but-empty credential, or no credential at all. Each would otherwise surface as an opaque TLS handshake
 * failure or an unauthenticated forward, long after the cause.
 */
export function parseCaMaterials(raw: string): CaMaterials {
  if (raw.trim() === "") {
    throw new CaError(
      "no material on stdin; the host generation step must pipe the certificates " +
        "and bearer into this container",
    );
  }

  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch (error) {
    // A truncated stream lands here, which is the point: the previous
    // marker-based format could accept a partial document.
    throw new CaError(`material is not valid JSON: ${(error as Error).message}`);
  }

  const document = asRecord(parsed, "material");

  if (document.schema !== MATERIAL_SCHEMA) {
    throw new CaError(
      `material schema ${JSON.stringify(document.schema)} does not match this ` +
        `bundle's ${JSON.stringify(MATERIAL_SCHEMA)}; refusing to start`,
    );
  }

  const caCertPem = requirePem(
    decodeBase64(document, "ca_cert", "material.ca_cert"),
    CERTIFICATE,
    "material.ca_cert",
  );

  // Starting without any credential would mean every allowed request is
  // forwarded unauthenticated, and Azure DevOps answers those with a sign-in
  // page a client can mistake for data. Which credential each configured family
  // needs is cross-checked against the policy by the caller.
  const token = optionalCredential(document, "token");
  const packageToken = optionalCredential(document, "package_token");
  if (token === undefined && packageToken === undefined) {
    throw new CaError(
      "material carries neither token nor package_token; refusing to start without a credential",
    );
  }

  const leavesDocument = asRecord(document.leaves, "material.leaves");
  const leaves = new Map<string, Leaf>();
  for (const [rawHost, value] of Object.entries(leavesDocument)) {
    const host = rawHost.trim().toLowerCase();
    if (host === "") throw new CaError("material.leaves has an empty hostname");
    const leaf = asRecord(value, `material.leaves[${host}]`);
    leaves.set(host, {
      key: requirePem(
        decodeBase64(leaf, "key", `material.leaves[${host}].key`),
        PRIVATE_KEY,
        `material.leaves[${host}].key`,
      ),
      cert: requirePem(
        decodeBase64(leaf, "cert", `material.leaves[${host}].cert`),
        CERTIFICATE,
        `material.leaves[${host}].cert`,
      ),
    });
  }

  if (leaves.size === 0) {
    throw new CaError("material carried no host leaves");
  }

  return {
    caCertPem,
    leaves,
    ...(token === undefined ? {} : { token }),
    ...(packageToken === undefined ? {} : { packageToken }),
  };
}

/**
 * Decode an optional credential field.
 *
 * Absent is fine; *present but empty* is not. An empty value means the host
 * step meant to supply a credential and failed to, which must surface here
 * rather than as an unauthenticated forward later.
 */
function optionalCredential(
  document: Record<string, unknown>,
  key: string,
): string | undefined {
  if (document[key] === undefined) return undefined;
  const value = decodeBase64(document, key, `material.${key}`).trim();
  if (value === "") throw new CaError(`material.${key} decoded to nothing`);
  return value;
}

/**
 * Read the material from a file descriptor, defaulting to stdin.
 *
 * Read once at startup and held in memory only. A restart therefore has no
 * material and fails closed, which is intended: a fresh CA would not be trusted
 * by the already-running MCP, so continuing would break every intercepted
 * request in a way that looks like a policy error rather than a restart.
 */
export function readCaMaterials(fd: number = 0): CaMaterials {
  let raw: string;
  try {
    raw = readFileSync(fd, "utf8");
  } catch (error) {
    throw new CaError(`cannot read material: ${(error as Error).message}`);
  }
  return parseCaMaterials(raw);
}

/**
 * Publish the CA certificate where the MCP container can mount it.
 *
 * Only the public certificate is ever written out; the private keys and the
 * bearer stay in this process.
 */
export function publishCaCertificate(path: string, caCertPem: string): void {
  if (PRIVATE_KEY.test(caCertPem)) {
    // Defence in depth: this path is mounted into another container, so a key
    // reaching it would hand out the ability to impersonate any protected host.
    throw new CaError("refusing to publish certificate material containing a private key");
  }
  writeFileSync(path, caCertPem, { mode: 0o644 });
  // `mode` is filtered through the process umask. The container deliberately
  // starts under `umask 077` so any accidentally-created private material is
  // owner-only; that also turns the public CA into 0600 unless we explicitly
  // correct it after creation. The MCP mount and the non-root AWF agent both
  // need read access to this certificate.
  chmodSync(path, 0o644);
}

/**
 * Cross-check the supplied credentials and leaves against the policy.
 *
 * Returns a reason to refuse to start, or `undefined`. Each family the policy
 * enables must have its own credential: starting without it would turn every
 * allowed request into a `502`, which reads to the agent as a flaky network
 * rather than a misconfigured pipeline. A package host also needs a leaf, or
 * every package request would fail at the TLS handshake with no explanation.
 */
export function credentialProblem(policy: ProxyPolicy, ca: CaMaterials): string | undefined {
  if (policy.capabilities.length > 0 && ca.token === undefined) {
    return (
      "the policy enables Azure DevOps REST capabilities but the material carries " +
      "no REST token"
    );
  }
  if (policy.packages !== undefined) {
    if (ca.packageToken === undefined) {
      return "the policy grants package feeds but the material carries no package_token";
    }
    for (const host of policy.packages.hosts) {
      if (!ca.leaves.has(canonicalizeHost(host))) {
        return `the policy grants package feeds but the material has no leaf for ${host}`;
      }
    }
  }
  return undefined;
}
