/**
 * The proxy server: two paths, chosen by destination host.
 *
 *   - **Not protected** — CONNECT straight through Squid and byte-tunnel. No
 *     TLS termination, no parsing, no header rewriting, so package feeds, model
 *     endpoints, and every other allowed host behave exactly as they do without
 *     this sidecar in the chain.
 *   - **Protected** — terminate TLS with the ephemeral CA, authorize against the
 *     catalog, strip client credentials, inject the bearer, forward through
 *     Squid, and filter the response.
 *
 * The proxy is deliberately safe when reached *directly* by the agent rather
 * than via Squid: AWF's internal network makes all peers mutually reachable, so
 * source address is not an authorization input. Policy is identical either way,
 * and there is no generic relay — an unprotected destination is tunnelled to
 * Squid, which applies its own domain policy, rather than dialled directly.
 */
import { randomUUID } from "node:crypto";
import { createServer as createHttpServer, type IncomingMessage, type ServerResponse, request as httpRequest } from "node:http";
import type { Server } from "node:http";
import type { Socket } from "node:net";
import { connect as tlsConnect, createSecureContext, createServer as createTlsServer, type TLSSocket } from "node:tls";

import type { CaMaterials } from "./ca.js";
import { canonicalizeHost, isProtectedHost } from "./catalog.js";
import type { ProxyConfig } from "./config.js";
import { sanitizeRequestHeaders, sanitizeResponseHeaders } from "./headers.js";
import { DecisionLog, statusClass, type DecisionRecord } from "./log.js";
import { authorize } from "./policy.js";
import { filterResponse } from "./response.js";
import { TokenError, TokenSource, bearerHeader } from "./token.js";
import { NormalizeError, normalizeTarget } from "./route.js";
import { connectThroughProxy, parseUpstreamProxy } from "./upstream.js";

/** Status returned for a policy denial. */
const DENY_STATUS = 403;
/**
 * Status returned when the proxy itself is broken (no token, upstream refused).
 *
 * Deliberately *not* 401, 429, or 503: `msrest` — which `az devops` uses —
 * retries those, turning one denied call into several and, for a semantic POST,
 * risking repeated side effects upstream. 502 is terminal for every client in
 * the supported set.
 */
const INFRA_STATUS = 502;

/**
 * Origin-form readiness path.
 *
 * AWF polls this before starting the agent, so the agent cannot race a proxy
 * that has not finished minting its CA. It is the only origin-form request the
 * proxy answers; everything else on that shape is a relay attempt.
 */
export const HEALTH_PATH = "/_ado-proxy/healthz";

/**
 * Cap on a single upstream request.
 *
 * A hung Azure DevOps connection would otherwise hold the agent's request — and
 * a socket — indefinitely, which reads to the agent as a stall rather than a
 * failure it can report.
 */
const UPSTREAM_TIMEOUT_MS = 120_000;

export interface ProxyDeps {
  readonly config: ProxyConfig;
  readonly ca: CaMaterials;
  readonly tokens: TokenSource;
  readonly log: DecisionLog;
  /**
   * Extra CA used when verifying the *upstream* Azure DevOps certificate.
   *
   * Only integration tests set this, to point the proxy at a fake upstream.
   * `rejectUnauthorized` stays on either way, so this narrows what the proxy
   * trusts rather than disabling verification.
   */
  readonly upstreamCa?: string;
}

/** Send a small JSON error body that no supported client will retry. */
function respondError(
  response: ServerResponse,
  status: number,
  reason: string,
  detail: string,
): void {
  // Azure DevOps `WrappedException` shape. `az` and every msrest-based SDK read
  // `message` when a call fails, so a denial surfaces as an actionable sentence
  // rather than "unexpected response".
  const body = JSON.stringify({
    $id: "1",
    innerException: null,
    message: `ado-proxy: ${detail}`,
    typeName: `AdoProxy.${reason}, ado-proxy`,
    typeKey: reason,
    errorCode: 0,
    eventId: 0,
  });
  response.writeHead(status, {
    "content-type": "application/json; charset=utf-8",
    "content-length": Buffer.byteLength(body),
    connection: "close",
  });
  response.end(body);
}

function hostAndPort(authority: string): { host: string; port: number } {
  const lastColon = authority.lastIndexOf(":");
  if (lastColon === -1 || authority.endsWith("]")) {
    return { host: canonicalizeHost(authority), port: 443 };
  }
  const port = Number(authority.slice(lastColon + 1));
  return {
    host: canonicalizeHost(authority.slice(0, lastColon)),
    port: Number.isInteger(port) && port > 0 ? port : 443,
  };
}

/**
 * Forward an absolute-form plain HTTP request to Squid.
 *
 * The agent's `HTTP_PROXY` points here, so *all* cleartext traffic arrives at
 * this handler — including `http://` package sources. Refusing it would be a
 * silent network regression, so it is relayed to Squid verbatim and Squid's own
 * domain policy decides. Nothing is inspected and no credential is added:
 * cleartext is never a protected path.
 */
function forwardPlainHttp(
  deps: ProxyDeps,
  request: IncomingMessage,
  response: ServerResponse,
): void {
  const proxy = parseUpstreamProxy(deps.config.upstreamProxy);
  const headers: Record<string, string | string[]> = {};
  for (const [name, value] of Object.entries(request.headers)) {
    // `connection` and friends are hop-by-hop; forwarding them would confuse
    // Squid about the lifetime of its own socket.
    if (["connection", "proxy-connection", "keep-alive"].includes(name.toLowerCase())) {
      continue;
    }
    if (value !== undefined) headers[name] = value;
  }

  const upstream = httpRequest({
    host: proxy.host,
    port: proxy.port,
    method: request.method,
    // Absolute-form target: this is exactly what a client with `HTTP_PROXY`
    // set would have sent to Squid directly.
    path: request.url ?? "/",
    headers,
  });

  upstream.on("response", (upstreamResponse) => {
    response.writeHead(upstreamResponse.statusCode ?? 502, upstreamResponse.headers);
    upstreamResponse.pipe(response);
  });
  upstream.on("error", () => {
    if (!response.headersSent) {
      respondError(response, INFRA_STATUS, "upstream-failed", "the upstream proxy failed");
      return;
    }
    response.destroy();
  });
  request.pipe(upstream);
}

async function tunnel(
  deps: ProxyDeps,
  clientSocket: Socket,
  head: Buffer,
  host: string,
  port: number,
): Promise<void> {
  const proxy = parseUpstreamProxy(deps.config.upstreamProxy);
  try {
    const upstream = await connectThroughProxy(proxy, host, port);
    if (clientSocket.destroyed) {
      // The client gave up while we were dialling Squid; do not leave the
      // upstream socket dangling.
      upstream.destroy();
      return;
    }
    clientSocket.write("HTTP/1.1 200 Connection Established\r\n\r\n");
    if (head.length > 0) upstream.write(head);
    upstream.pipe(clientSocket);
    clientSocket.pipe(upstream);
    const destroyBoth = (): void => {
      upstream.destroy();
      clientSocket.destroy();
    };
    upstream.on("error", destroyBoth);
    clientSocket.on("error", destroyBoth);
    clientSocket.on("close", destroyBoth);
  } catch (error) {
    // Mirror Squid's own refusal shape rather than inventing one; the client
    // sees the same failure it would see talking to Squid directly.
    if (!clientSocket.destroyed) clientSocket.end("HTTP/1.1 502 Bad Gateway\r\n\r\n");
    deps.log.write({
      ts: new Date().toISOString(),
      request_id: randomUUID(),
      host,
      method: "CONNECT",
      decision: "error",
      reason: "tunnel-failed",
      detail: (error as Error).message,
    });
  }
}

/** Read a bounded response body, destroying the stream if the cap is passed. */
function readBounded(stream: IncomingMessage, limit: number): Promise<Buffer> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = [];
    let total = 0;
    stream.on("data", (chunk: Buffer) => {
      total += chunk.length;
      if (total > limit) {
        stream.destroy();
        reject(new Error(`upstream response exceeded ${limit} bytes`));
        return;
      }
      chunks.push(chunk);
    });
    stream.on("end", () => resolve(Buffer.concat(chunks)));
    stream.on("error", reject);
  });
}

/**
 * Handle one intercepted request to a protected host.
 *
 * Order is the security contract: normalize, authorize, *then* read the token.
 * A denial therefore never touches the credential, and no code path can emit
 * the bearer for a request that was not fully approved.
 */
async function handleProtected(
  deps: ProxyDeps,
  host: string,
  request: IncomingMessage,
  response: ServerResponse,
): Promise<void> {
  const started = Date.now();
  const requestId = randomUUID();
  const method = request.method ?? "GET";
  const base: Omit<DecisionRecord, "decision"> = {
    ts: new Date().toISOString(),
    request_id: requestId,
    host,
    method,
  };

  let target;
  try {
    target = normalizeTarget(request.url ?? "/");
  } catch (error) {
    if (!(error instanceof NormalizeError)) throw error;
    deps.log.write({ ...base, decision: "deny", reason: "malformed-target", detail: error.message });
    respondError(response, DENY_STATUS, "malformed-target", error.message);
    return;
  }

  const accept = request.headers.accept;
  const decision = authorize(
    { method, host, target, accept: Array.isArray(accept) ? accept[0] : accept },
    deps.config.policy,
  );

  if (!decision.allow) {
    deps.log.write({
      ...base,
      decision: "deny",
      reason: decision.reason,
      detail: decision.detail,
      ...(decision.operationId === undefined ? {} : { operation: decision.operationId }),
    });
    respondError(response, DENY_STATUS, decision.reason, decision.detail);
    return;
  }

  let token: string;
  try {
    token = deps.tokens.read();
  } catch (error) {
    if (!(error instanceof TokenError)) throw error;
    deps.log.write({
      ...base,
      operation: decision.operation.id,
      decision: "error",
      reason: "credential-unavailable",
      detail: "the proxy has no current Azure DevOps token",
    });
    // Never forward an authorized request unauthenticated: Azure DevOps would
    // answer with a sign-in page that clients cannot distinguish from data.
    respondError(
      response,
      INFRA_STATUS,
      "credential-unavailable",
      "the ado-proxy has no current Azure DevOps credential",
    );
    return;
  }

  const { headers, strippedCredentials } = sanitizeRequestHeaders(request.headers, host);
  // The single point at which the real credential enters a request. It is
  // applied to a *copy*, after the allow decision and after the token read, so
  // no earlier code path can observe or emit it.
  const upstreamHeaders: Record<string, string> = {
    ...headers,
    authorization: bearerHeader(token),
  };

  const proxy = parseUpstreamProxy(deps.config.upstreamProxy);
  let secured: TLSSocket | undefined;
  try {
    const raw = await connectThroughProxy(proxy, host, 443);
    secured = tlsConnect({
      socket: raw,
      servername: host,
      ...(deps.upstreamCa === undefined ? {} : { ca: deps.upstreamCa }),
    });
    const upstreamSocket = secured;
    await new Promise<void>((resolve, reject) => {
      upstreamSocket.once("secureConnect", () => resolve());
      upstreamSocket.once("error", reject);
    });
    // A late socket error — after the handshake, or after the response has been
    // read — would otherwise be an unhandled 'error' event and crash the proxy,
    // taking down the agent's only route to Azure DevOps.
    upstreamSocket.on("error", () => upstreamSocket.destroy());

    const upstream = httpRequest({
      // No `agent` key at all: Node only honours `createConnection` when the
      // agent is left undefined. Setting `agent: false` makes it construct a
      // default agent, which would ignore this socket and dial localhost.
      createConnection: () => upstreamSocket,
      method,
      path: request.url ?? "/",
      headers: upstreamHeaders,
    });
    upstream.setTimeout(UPSTREAM_TIMEOUT_MS, () => {
      upstream.destroy(new Error("upstream request timed out"));
    });

    const upstreamResponse = await new Promise<IncomingMessage>((resolve, reject) => {
      upstream.once("response", resolve);
      upstream.once("error", reject);
      upstream.end();
    });
    upstream.on("error", () => upstream.destroy());

    const body = await readBounded(upstreamResponse, decision.operation.max_response_bytes);
    const status = upstreamResponse.statusCode ?? 502;
    const outcome = filterResponse(decision.operation, deps.config.policy, body);

    if (outcome.kind === "deny") {
      deps.log.write({
        ...base,
        operation: decision.operation.id,
        decision: "deny",
        reason: "out-of-scope-response",
        detail: outcome.detail,
        upstream_status_class: statusClass(status),
        latency_ms: Date.now() - started,
      });
      respondError(response, DENY_STATUS, "out-of-scope-response", outcome.detail);
      return;
    }

    const responseHeaders = sanitizeResponseHeaders(upstreamResponse.headers);
    responseHeaders["content-length"] = String(outcome.body.length);
    responseHeaders.connection = "close";
    response.writeHead(status, responseHeaders);
    response.end(outcome.body);

    deps.log.write({
      ...base,
      operation: decision.operation.id,
      decision: "allow",
      upstream_status_class: statusClass(status),
      latency_ms: Date.now() - started,
      response_bytes: outcome.body.length,
      ...(strippedCredentials.length === 0
        ? {}
        : { stripped_credentials: strippedCredentials }),
    });
  } catch (error) {
    secured?.destroy();
    deps.log.write({
      ...base,
      operation: decision.operation.id,
      decision: "error",
      reason: "upstream-failed",
      detail: (error as Error).message,
      latency_ms: Date.now() - started,
    });
    if (!response.headersSent) {
      respondError(response, INFRA_STATUS, "upstream-failed", "the upstream request failed");
      return;
    }
    // The body was already committed; the only safe move is to cut the
    // connection rather than append anything to a partially sent response.
    response.destroy();
  }
}

/**
 * Build the intercepting HTTPS front end.
 *
 * One TLS server serves every protected host, selecting the right leaf by SNI.
 * Sockets are handed to an inner HTTP server so Node parses the tunnelled
 * requests for us.
 */
function createInterceptor(deps: ProxyDeps): (socket: Socket, host: string) => void {
  const inner = createHttpServer((request, response) => {
    const socket = request.socket as TLSSocket;
    const host = canonicalizeHost(socket.servername || "");
    void handleProtected(deps, host, request, response).catch(() => {
      if (!response.headersSent) {
        respondError(response, INFRA_STATUS, "internal-error", "the proxy failed");
      }
    });
  });

  const tls = createTlsServer({
    // HTTP/1.1 only. The inner server is an `http.Server`, which cannot parse
    // an h2 stream; without pinning ALPN a modern client would negotiate h2 and
    // then talk a protocol nothing here understands.
    ALPNProtocols: ["http/1.1"],
    SNICallback: (servername, callback) => {
      const leaf = deps.ca.leaves.get(canonicalizeHost(servername));
      if (leaf === undefined) {
        // Only compiler-pinned protected hosts have leaves. Anything else
        // reaching the interceptor is a mismatch between the CONNECT target and
        // the SNI, which is a smuggling attempt, not a supported client.
        callback(new Error(`no certificate for ${servername}`));
        return;
      }
      callback(null, createSecureContext({ key: leaf.key, cert: leaf.cert }));
    },
  });
  tls.on("secureConnection", (socket) => inner.emit("connection", socket));

  return (socket: Socket, host: string) => {
    if (!deps.ca.leaves.has(host)) {
      socket.destroy();
      return;
    }
    tls.emit("connection", socket);
  };
}

/** Create the listening proxy server. */
export function createProxyServer(deps: ProxyDeps): Server {
  const intercept = createInterceptor(deps);

  const server = createHttpServer((request, response) => {
    const target = request.url ?? "";

    if (target === HEALTH_PATH) {
      // AWF waits for this before starting the agent, so that the agent never
      // races a proxy that has not yet minted its CA. Deliberately reveals no
      // policy detail.
      const body = JSON.stringify({ status: "ok" });
      response.writeHead(200, {
        "content-type": "application/json",
        "content-length": Buffer.byteLength(body),
        connection: "close",
      });
      response.end(body);
      return;
    }

    if (!target.startsWith("http://")) {
      // Origin-form on the proxy port means someone addressed the proxy itself
      // rather than asking it to reach a destination. There is nothing here to
      // serve, and answering would make this look like a generic relay.
      respondError(
        response,
        DENY_STATUS,
        "relay-denied",
        "this proxy serves absolute-form requests and CONNECT only",
      );
      return;
    }

    let host: string;
    try {
      host = canonicalizeHost(new URL(target).hostname);
    } catch {
      respondError(response, DENY_STATUS, "malformed-target", "unparseable request target");
      return;
    }

    if (isProtectedHost(host)) {
      // Azure DevOps is HTTPS-only. A cleartext request to a protected host is
      // a downgrade attempt, and relaying it would put the policy path — and
      // therefore the bearer — on an unencrypted hop.
      respondError(
        response,
        DENY_STATUS,
        "cleartext-denied",
        "protected Azure DevOps hosts must be reached over HTTPS",
      );
      return;
    }

    forwardPlainHttp(deps, request, response);
  });

  server.on("connect", (request, socket: Socket, head: Buffer) => {
    // Node removes its own `error` listener from the socket before emitting
    // `connect`, and `clientError` does not cover sockets it has handed off.
    // Without this, a client that resets the connection while we are still
    // dialling Squid produces an uncaught exception and kills the sidecar —
    // the agent's only route to Azure DevOps.
    socket.on("error", () => socket.destroy());

    const { host, port } = hostAndPort(request.url ?? "");
    if (isProtectedHost(host)) {
      if (port !== 443) {
        // Azure DevOps serves REST on 443 only. A protected host on another
        // port would skip interception here and land on Squid's generic
        // domain policy, which is not scoped to the catalog.
        socket.end("HTTP/1.1 403 Forbidden\r\n\r\n");
        return;
      }
      if (head.length > 0) socket.unshift(head);
      socket.write("HTTP/1.1 200 Connection Established\r\n\r\n");
      intercept(socket, host);
      return;
    }
    void tunnel(deps, socket, head, host, port);
  });

  // A client that vanishes mid-handshake must not take the proxy down with it.
  server.on("clientError", (_error, socket) => {
    (socket as Socket).destroy();
  });

  return server;
}
