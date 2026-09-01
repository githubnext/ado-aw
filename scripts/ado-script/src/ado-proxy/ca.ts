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
 *   "schema": "ado-aw/ado-proxy-material/v1",
 *   "ca_cert": "<base64 PEM>",
 *   "token": "<base64>",
 *   "leaves": { "dev.azure.com": { "key": "<base64 PEM>", "cert": "<base64 PEM>" } }
 * }
 * ```
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

export class CaError extends Error {}

/** Wire-format version, checked on parse so a mismatch fails closed. */
export const MATERIAL_SCHEMA = "ado-aw/ado-proxy-material/v1";

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
   * The Azure DevOps bearer.
   *
   * Carried in the same document as the certificates because it has the same
   * custody requirement: it must reach this process without touching a path
   * the agent can read.
   */
  readonly token: string;
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
 * missing CA, a host without both a key and a certificate, no hosts at all, or
 * a missing bearer. Each would otherwise surface as an opaque TLS handshake
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

  // Starting without a bearer would mean every allowed request is forwarded
  // unauthenticated, and Azure DevOps answers those with a sign-in page a
  // client can mistake for data.
  const token = decodeBase64(document, "token", "material.token").trim();

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

  return { caCertPem, leaves, token };
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
