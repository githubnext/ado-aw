/**
 * Interception certificate material, supplied on stdin.
 *
 * The engine does **not** mint its own certificates. A host pipeline step runs
 * `openssl` — already an unconditional dependency of every compiled pipeline,
 * which mints the MCPG API key with `openssl rand` — and pipes the CA plus one
 * leaf per protected host straight into `docker run -i`. Two consequences:
 *
 *   - **The private keys touch no filesystem.** Not the runner's, not the
 *     container's. AWF's chroot makes the agent's root the host's `/host` bind
 *     mount, so the agent's `/tmp` *is* the runner's `/tmp`; a key written to a
 *     host path would be agent-readable. Keeping it on stdin sidesteps that
 *     rather than relying on deleting it in time. There is no exposure window
 *     to get wrong either, because the engine starts before AWF — at generation
 *     time no agent exists at all.
 *   - **The engine needs no `openssl`,** so it runs on `node:20-slim` (which
 *     has none) rather than the full `node:20`. That is already the image the
 *     Azure DevOps MCP uses, so it adds nothing to mirror.
 *
 * The protected host set is compiler-known, so every leaf can be generated
 * ahead of time. Nothing here has to *issue* a certificate — fortunate, since
 * Node can parse X.509 but not issue it.
 */
import { readFileSync, writeFileSync } from "node:fs";

export class CaError extends Error {}

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
   * Carried in the same stream as the certificates because it has the same
   * custody requirement: it must reach this process without touching a path
   * the agent can read.
   */
  readonly token: string;
}

/**
 * Section markers in the piped stream.
 *
 * A marker format rather than bare PEM concatenation, because each leaf's key
 * and certificate must stay associated with *its* hostname — relying on order
 * alone would be a silent correctness trap if the generator ever changed.
 */
const CA_MARKER = "### CA";
const HOST_MARKER = "### HOST ";
const TOKEN_MARKER = "### TOKEN";


const PRIVATE_KEY =
  /-----BEGIN (?:RSA |EC )?PRIVATE KEY-----[\s\S]*?-----END (?:RSA |EC )?PRIVATE KEY-----/;
const CERTIFICATE = /-----BEGIN CERTIFICATE-----[\s\S]*?-----END CERTIFICATE-----/;

/**
 * Parse the certificate stream.
 *
 * Fails closed on anything incomplete: a missing CA, a host section without
 * both a key and a certificate, or a stream carrying no hosts at all. Each
 * would otherwise surface as an opaque TLS handshake failure on the first
 * intercepted request, long after the cause.
 */
export function parseCaMaterials(raw: string): CaMaterials {
  if (raw.trim() === "") {
    throw new CaError(
      "no certificate material on stdin; the host generation step must pipe the " +
        "CA and leaves into this container",
    );
  }

  const sections = raw.split("### ").slice(1);
  let caCertPem: string | undefined;
  let token: string | undefined;
  const leaves = new Map<string, Leaf>();

  for (const section of sections) {
    const body = `### ${section}`;

    if (body.startsWith(CA_MARKER)) {
      const cert = CERTIFICATE.exec(body)?.[0];
      if (cert === undefined) throw new CaError("CA section carried no certificate");
      caCertPem = cert;
      continue;
    }

    if (body.startsWith(TOKEN_MARKER)) {
      const newline = body.indexOf("\n");
      token = newline === -1 ? "" : body.slice(newline + 1).trim();
      continue;
    }

    if (!body.startsWith(HOST_MARKER)) continue;


    const newline = body.indexOf("\n");
    const host = body
      .slice(HOST_MARKER.length, newline === -1 ? undefined : newline)
      .trim()
      .toLowerCase();
    if (host === "") throw new CaError("host section carried no hostname");

    const key = PRIVATE_KEY.exec(body)?.[0];
    const cert = CERTIFICATE.exec(body)?.[0];
    if (key === undefined || cert === undefined) {
      // Half a leaf is worse than none: TLS would fail at handshake time with
      // nothing to indicate the *material* was the problem.
      throw new CaError(
        `leaf for ${host} is missing its ${key === undefined ? "key" : "certificate"}`,
      );
    }
    leaves.set(host, { key, cert });
  }

  if (caCertPem === undefined) {
    throw new CaError("certificate stream carried no CA section");
  }
  if (leaves.size === 0) {
    throw new CaError("certificate stream carried no host leaves");
  }
  if (token === undefined || token === "") {
    // Starting without a bearer would mean every allowed request is forwarded
    // unauthenticated, and Azure DevOps answers those with a sign-in page a
    // client can mistake for data. Refuse instead.
    throw new CaError("stream carried no Azure DevOps bearer");
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
    throw new CaError(`cannot read certificate material: ${(error as Error).message}`);
  }
  return parseCaMaterials(raw);
}

/**
 * Publish the CA certificate where the MCP container can mount it.
 *
 * Only the public certificate is ever written out; the private keys stay in
 * this process.
 */
export function publishCaCertificate(path: string, caCertPem: string): void {
  if (PRIVATE_KEY.test(caCertPem)) {
    // Defence in depth: this path is mounted into another container, so a key
    // reaching it would hand out the ability to impersonate any protected host.
    throw new CaError("refusing to publish certificate material containing a private key");
  }
  writeFileSync(path, caCertPem, { mode: 0o644 });
}
