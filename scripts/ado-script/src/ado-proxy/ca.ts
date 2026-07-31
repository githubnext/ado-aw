/**
 * Ephemeral interception CA and per-host leaf certificates.
 *
 * Node cannot *create* X.509 certificates: `node:crypto` can generate key pairs
 * and parse certificates, but has no issuance API. The options are a native
 * crypto dependency (which is what pushed this runtime off Rust in the first
 * place) or the `openssl` binary that is already present in the AWF agent image
 * and already used by AWF's own ssl-bump setup. This module takes the second
 * path, so the bundle keeps zero runtime dependencies.
 *
 * Key custody: every private key is written under a caller-supplied directory
 * that must be container tmpfs. Only the CA's *public* certificate is ever
 * copied out, into the pre-created file AWF installs into the agent's trust
 * stores.
 */
import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

export class CaError extends Error {}

/** A minted leaf certificate for one protected host. */
export interface Leaf {
  readonly key: string;
  readonly cert: string;
}

/** Materials produced by {@link mintCa}. */
export interface CaMaterials {
  /** PEM of the CA certificate. Safe to publish. */
  readonly caCertPem: string;
  /** Leaf key/cert per protected host, keyed by lowercase hostname. */
  readonly leaves: ReadonlyMap<string, Leaf>;
}

const DAYS = "2";
const SUBJECT = "/CN=ado-proxy ephemeral interception CA";

function openssl(args: readonly string[], cwd: string): void {
  try {
    execFileSync("openssl", args as string[], {
      cwd,
      stdio: ["ignore", "ignore", "pipe"],
      timeout: 60_000,
    });
  } catch (error) {
    const stderr = (error as { stderr?: Buffer }).stderr?.toString().trim();
    throw new CaError(
      `openssl ${args[0]} failed${stderr === undefined || stderr === "" ? "" : `: ${stderr}`}`,
    );
  }
}

/**
 * Generate a fresh CA and one leaf per protected host.
 *
 * All hosts are known at startup — the protected set is compiler-pinned and
 * tiny — so leaves are minted eagerly. That keeps `openssl` off the request
 * path entirely and means a broken toolchain fails at startup rather than on
 * the first intercepted connection.
 */
export function mintCa(
  directory: string,
  hosts: readonly string[],
): CaMaterials {
  mkdirSync(directory, { recursive: true, mode: 0o700 });

  openssl(
    [
      "req",
      "-x509",
      "-newkey",
      "rsa:2048",
      "-nodes",
      "-days",
      DAYS,
      "-subj",
      SUBJECT,
      "-keyout",
      "ca.key",
      "-out",
      "ca.pem",
      "-addext",
      "basicConstraints=critical,CA:TRUE,pathlen:0",
      "-addext",
      "keyUsage=critical,keyCertSign,cRLSign",
    ],
    directory,
  );

  const leaves = new Map<string, Leaf>();
  for (const rawHost of hosts) {
    const host = rawHost.toLowerCase();
    if (leaves.has(host)) continue;
    if (!/^[a-z0-9.-]+$/.test(host)) {
      // The protected set is compiler-owned, but this string ends up in an
      // openssl config file; refuse anything that could break out of it.
      throw new CaError(`refusing to mint a certificate for host ${rawHost}`);
    }

    const keyFile = `${host}.key`;
    const csrFile = `${host}.csr`;
    const certFile = `${host}.pem`;
    const extFile = `${host}.ext`;

    writeFileSync(
      join(directory, extFile),
      [
        "basicConstraints=CA:FALSE",
        "keyUsage=critical,digitalSignature,keyEncipherment",
        "extendedKeyUsage=serverAuth",
        `subjectAltName=DNS:${host}`,
        "",
      ].join("\n"),
      { mode: 0o600 },
    );

    openssl(
      [
        "req",
        "-new",
        "-newkey",
        "rsa:2048",
        "-nodes",
        "-subj",
        `/CN=${host}`,
        "-keyout",
        keyFile,
        "-out",
        csrFile,
      ],
      directory,
    );

    openssl(
      [
        "x509",
        "-req",
        "-in",
        csrFile,
        "-CA",
        "ca.pem",
        "-CAkey",
        "ca.key",
        "-CAcreateserial",
        "-days",
        DAYS,
        "-extfile",
        extFile,
        "-out",
        certFile,
      ],
      directory,
    );

    leaves.set(host, {
      key: readFileSync(join(directory, keyFile), "utf8"),
      cert: readFileSync(join(directory, certFile), "utf8"),
    });
  }

  return {
    caCertPem: readFileSync(join(directory, "ca.pem"), "utf8"),
    leaves,
  };
}

/**
 * Publish the CA certificate where AWF expects it.
 *
 * AWF pre-creates this path as a regular file before the sidecar starts, so a
 * symlink cannot be swapped in between creation and write. Only the public
 * certificate is ever written; the private key stays in the tmpfs directory.
 */
export function publishCaCertificate(path: string, caCertPem: string): void {
  writeFileSync(path, caCertPem, { mode: 0o644 });
}
