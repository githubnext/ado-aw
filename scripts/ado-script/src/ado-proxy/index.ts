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
 * file the trusted host task rotates. The interception certificates arrive on
 * stdin from a host generation step, so their private keys touch no filesystem;
 * only the *public* CA certificate is ever written out.
 *
 * Unlike the other `ado-script` bundles, which are short-lived pipeline steps,
 * this one is a long-running server: it starts before the agent and is torn
 * down by AWF when the agent exits.
 */
import { CaError, publishCaCertificate, readCaMaterials } from "./ca.js";
import { ConfigError, loadConfig, type ProxyConfig } from "./config.js";
import { DecisionLog } from "./log.js";
import { createDirectTlsServer, createProxyServer } from "./server.js";
import { TokenSource } from "./token.js";
import { UpstreamError, parseUpstreamProxy } from "./upstream.js";

function report(message: string): void {
  process.stderr.write(`[ado-proxy] ${message}\n`);
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
    // Read before anything else binds a port: without an interception identity
    // there is nothing safe to serve, and the agent's clients would reject
    // interception anyway. The only "fix" for that would be to stop
    // intercepting, which is exactly what this proxy exists to prevent.
    ca = readCaMaterials();
    publishCaCertificate(config.publicCaFile, ca.caCertPem);
  } catch (error) {
    if (!(error instanceof CaError)) throw error;
    report(`cannot establish the interception identity: ${error.message}`);
    return 1;
  }

  const deps = {
    config,
    ca,
    tokens: new TokenSource(ca.token),
    log: new DecisionLog(config.logDir),
  };
  const server = createProxyServer(deps);
  const tlsServer = createDirectTlsServer(deps);

  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(config.listenPort, config.listenAddress, resolve);
  });

  // The direct-TLS listener is what the redirected MCP and the `az` wrapper
  // actually use; the proxy-style listener above remains for CONNECT clients.
  await new Promise<void>((resolve, reject) => {
    tlsServer.once("error", reject);
    tlsServer.listen(config.tlsPort, config.listenAddress, resolve);
  });

  report(
    `listening on ${config.listenAddress}:${config.listenPort} (proxy) and ` +
      `${config.listenAddress}:${config.tlsPort} (direct TLS); ` +
      `org=${config.policy.organization} project=${config.policy.project} ` +
      `capabilities=${config.policy.capabilities.join(",") || "(none)"} ` +
      `protected=${config.policy.protected_hosts.join(",")}`,
  );

  // AWF stops the sidecar once the agent exits. Close politely so buffered
  // decision-log lines are flushed, but do not wait forever for a hung tunnel.
  await new Promise<void>((resolve) => {
    const shutdown = (signal: string): void => {
      report(`received ${signal}; shutting down`);
      let remaining = 2;
      const done = (): void => {
        remaining -= 1;
        if (remaining === 0) resolve();
      };
      server.close(done);
      tlsServer.close(done);
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
