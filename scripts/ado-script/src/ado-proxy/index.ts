/**
 * `ado-proxy` — the credential-isolated Azure DevOps policy proxy.
 *
 * AWF runs this bundle as a managed sidecar on the internal `awf-net`, points
 * the agent's `HTTP(S)_PROXY` at it, and denies the protected Azure DevOps
 * hosts to the agent at Squid. That makes this process the only path from the
 * agent to those hosts.
 *
 * Two request paths, and only two:
 *
 *   - **Non-protected destination** — CONNECT through Squid and byte-tunnel in
 *     both directions. No TLS termination, no parsing, no header changes, so
 *     package feeds and every other allowed host behave exactly as they do
 *     without the sidecar.
 *   - **Protected destination** — terminate TLS with an ephemeral CA, evaluate
 *     the request against the versioned catalog, strip every client-supplied
 *     credential, and inject the current bearer *only* after a complete allow
 *     decision.
 *
 * The bearer is never in argv or the environment: it is read from a private
 * file the trusted host task rotates. Only the *public* interception
 * certificate is ever written out.
 *
 * Unlike the other `ado-script` bundles, which are short-lived pipeline steps,
 * this one is a long-running server: it starts before the agent and is torn
 * down by AWF when the agent exits.
 */
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { CaError, mintCa, publishCaCertificate } from "./ca.js";
import { ConfigError, loadConfig, type ProxyConfig } from "./config.js";
import { DecisionLog } from "./log.js";
import { createProxyServer } from "./server.js";
import { TokenSource } from "./token.js";
import { UpstreamError, parseUpstreamProxy } from "./upstream.js";

function report(message: string): void {
  process.stderr.write(`[ado-proxy] ${message}\n`);
}

/**
 * Where the CA private key lives.
 *
 * AWF mounts tmpfs at this path so the key never touches a host filesystem or
 * any volume the agent can see. When it is absent — as in tests — fall back to
 * a private temporary directory rather than failing, since the key is
 * regenerated per process either way.
 */
function keyDirectory(): string {
  const configured = process.env.AWF_POLICY_PROXY_TMPFS_DIR;
  if (configured !== undefined && configured !== "") {
    return join(configured, "ado-proxy-ca");
  }
  return mkdtempSync(join(tmpdir(), "ado-proxy-ca-"));
}

/** Start the proxy and resolve with the process exit code once it stops. */
export async function run(argv: readonly string[]): Promise<number> {
  let config: ProxyConfig;
  try {
    config = loadConfig(argv);
  } catch (error) {
    if (!(error instanceof ConfigError)) throw error;
    // Fail closed and say why: a proxy that starts without a valid policy
    // would be an open tunnel to the protected hosts.
    report(`configuration error: ${error.message}`);
    return 1;
  }

  try {
    parseUpstreamProxy(config.upstreamProxy);
  } catch (error) {
    if (!(error instanceof UpstreamError)) throw error;
    report(`configuration error: ${error.message}`);
    return 1;
  }

  let ca;
  try {
    ca = mintCa(keyDirectory(), config.policy.protected_hosts);
    publishCaCertificate(config.publicCaFile, ca.caCertPem);
  } catch (error) {
    if (!(error instanceof CaError)) throw error;
    // Without a trusted CA the agent's clients reject interception, and the
    // only "fix" would be to stop intercepting — which is the thing this proxy
    // exists to prevent.
    report(`cannot establish the interception CA: ${error.message}`);
    return 1;
  }

  const server = createProxyServer({
    config,
    ca,
    tokens: new TokenSource(config.tokenFile),
    log: new DecisionLog(config.logDir),
  });

  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(config.listenPort, config.listenAddress, resolve);
  });

  report(
    `listening on ${config.listenAddress}:${config.listenPort}; ` +
      `org=${config.policy.organization} project=${config.policy.project} ` +
      `capabilities=${config.policy.capabilities.join(",") || "(none)"} ` +
      `protected=${config.policy.protected_hosts.join(",")}`,
  );

  // AWF stops the sidecar once the agent exits. Close politely so buffered
  // decision-log lines are flushed, but do not wait forever for a hung tunnel.
  await new Promise<void>((resolve) => {
    const shutdown = (signal: string): void => {
      report(`received ${signal}; shutting down`);
      server.close(() => resolve());
      setTimeout(resolve, 5_000).unref();
    };
    process.once("SIGTERM", () => shutdown("SIGTERM"));
    process.once("SIGINT", () => shutdown("SIGINT"));
  });

  return 0;
}

async function main(): Promise<void> {
  process.exitCode = await run(process.argv.slice(2));
}

void main();
