/**
 * End-to-end test of the proxy against a fake Squid and a fake Azure DevOps.
 *
 * The unit suites cover each decision in isolation; this one proves the wiring
 * that actually protects the credential:
 *
 *   - an allowed read reaches the upstream carrying the injected bearer, and
 *     the sentinel the client supplied is gone;
 *   - every denial is refused *before* the upstream is contacted, so a rejected
 *     request cannot consume, observe, or exercise the credential;
 *   - a non-protected destination is byte-tunnelled to Squid untouched, keeping
 *     its own certificate end to end;
 *   - a missing credential is an infrastructure failure, never an
 *     unauthenticated pass-through.
 *
 * The canary bearer is asserted absent from every response body the client
 * sees, so a future refactor that echoes upstream detail back to the agent
 * fails here.
 */
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer as createHttpServer, request as httpRequest, type Server } from "node:http";
import { connect as netConnect, type Socket } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  connect as tlsConnect,
  createServer as createTlsServer,
  type TlsOptions,
} from "node:tls";

import { afterAll, beforeAll, describe, expect, it } from "vitest";

import { parseCaMaterials, type CaMaterials } from "./ca.js";
import { CATALOG_SCHEMA_VERSION } from "./catalog.js";
import type { ProxyConfig, ProxyPolicy } from "./config.js";
import { DecisionLog } from "./log.js";
import { HEALTH_PATH, createProxyServer } from "./server.js";
import { TokenSource } from "./token.js";

/**
 * Locate `openssl`.
 *
 * CI runs on Linux where it is always on PATH. On a Windows dev machine it
 * usually ships with Git but is not exported, so look there too rather than
 * silently skipping the suite that proves the security boundary.
 */
function ensureOpenssl(): boolean {
  const candidates = [
    "C:\\Program Files\\Git\\usr\\bin",
    "C:\\Program Files\\Git\\mingw64\\bin",
  ];
  try {
    execFileSync("openssl", ["version"], { stdio: "ignore" });
    return true;
  } catch {
    for (const directory of candidates) {
      if (!existsSync(join(directory, "openssl.exe"))) continue;
      process.env.PATH = `${directory};${process.env.PATH ?? ""}`;
      try {
        execFileSync("openssl", ["version"], { stdio: "ignore" });
        return true;
      } catch {
        // Keep looking.
      }
    }
    return false;
  }
}

const CANARY = "canary-bearer-8f2c1d4e9a7b";
const SENTINEL = "ado-proxy-sentinel-not-a-credential";
const ORGANIZATION = "contoso";

const POLICY: ProxyPolicy = {
  catalog_version: CATALOG_SCHEMA_VERSION,
  organization: ORGANIZATION,
  project: "Widgets",
  project_id: "11111111-1111-1111-1111-111111111111",
  repository: "widget-api",
  repository_id: "22222222-2222-2222-2222-222222222222",
  capabilities: ["discovery", "core", "repos", "pipelines", "boards"],
  protected_hosts: ["dev.azure.com", "app.vssps.visualstudio.com"],
  allowed_resource_areas: [],
};

interface UpstreamCall {
  readonly method: string;
  readonly url: string;
  readonly authorization: string | undefined;
  readonly headerNames: readonly string[];
}

interface Harness {
  readonly proxyPort: number;
  readonly proxyCaPem: string;
  readonly upstreamCalls: UpstreamCall[];
  readonly tunnelTargets: string[];
  readonly tokenFile: string;
}

let workdir: string;
let harness: Harness;
const servers: { close(callback: () => void): void }[] = [];
const hasOpenssl = ensureOpenssl();

function listen(server: { listen: (...args: never[]) => void }): Promise<number> {
  return new Promise((resolve) => {
    (server as unknown as Server).listen(0, "127.0.0.1", () => {
      const address = (server as unknown as Server).address();
      resolve(typeof address === "object" && address !== null ? address.port : 0);
    });
  });
}

/**
 * Mint CA material in the stream format the engine consumes.
 *
 * The engine no longer mints its own certificates — a host pipeline step does,
 * and pipes them in — so this stands in for that step. It returns the parsed
 * form for the harness's own servers, and the raw stream for feeding the engine.
 */
function mintForTest(directory: string, hosts: readonly string[]): {
  materials: CaMaterials;
  stream: string;
} {
  mkdirSync(directory, { recursive: true });
  const run = (args: readonly string[]): void => {
    execFileSync("openssl", args as string[], {
      cwd: directory,
      stdio: ["ignore", "ignore", "pipe"],
    });
  };

  run([
    "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "2",
    "-subj", "/CN=ado-proxy test CA", "-keyout", "ca.key", "-out", "ca.pem",
    "-addext", "basicConstraints=critical,CA:TRUE,pathlen:0",
  ]);

  let stream = `### CA\n${readFileSync(join(directory, "ca.pem"), "utf8")}`;

  for (const host of hosts) {
    writeFileSync(join(directory, "leaf.ext"),
      "basicConstraints=CA:FALSE\n" +
      "keyUsage=critical,digitalSignature,keyEncipherment\n" +
      "extendedKeyUsage=serverAuth\n" +
      `subjectAltName=DNS:${host}\n`);
    run(["req", "-new", "-newkey", "rsa:2048", "-nodes", "-subj", `/CN=${host}`,
      "-keyout", "leaf.key", "-out", "leaf.csr"]);
    run(["x509", "-req", "-in", "leaf.csr", "-CA", "ca.pem", "-CAkey", "ca.key",
      "-CAcreateserial", "-days", "2", "-extfile", "leaf.ext", "-out", "leaf.pem"]);
    stream += `### HOST ${host}\n` +
      readFileSync(join(directory, "leaf.key"), "utf8") +
      readFileSync(join(directory, "leaf.pem"), "utf8");
  }

  return { materials: parseCaMaterials(stream), stream };
}

/** A TLS server standing in for `dev.azure.com`. */
async function startFakeAdo(ca: CaMaterials, calls: UpstreamCall[]): Promise<number> {
  const leaf = ca.leaves.get("dev.azure.com");
  if (leaf === undefined) throw new Error("fake upstream has no leaf");

  const app = createHttpServer((request, response) => {
    calls.push({
      method: request.method ?? "",
      url: request.url ?? "",
      authorization: request.headers.authorization,
      headerNames: Object.keys(request.headers),
    });
    const body = JSON.stringify({
      count: 2,
      value: [
        { id: POLICY.project_id, name: "Widgets" },
        { id: "33333333-3333-3333-3333-333333333333", name: "Secrets" },
      ],
    });
    response.writeHead(200, {
      "content-type": "application/json",
      "set-cookie": "UserAuthentication=should-not-reach-the-agent",
      "content-length": Buffer.byteLength(body),
    });
    response.end(body);
  });

  const options: TlsOptions = { key: leaf.key, cert: leaf.cert };
  const tls = createTlsServer(options);
  tls.on("secureConnection", (socket) => app.emit("connection", socket));
  servers.push(tls);
  return listen(tls as never);
}

/** A TLS server standing in for an ordinary, non-protected host. */
async function startPlainHost(ca: CaMaterials): Promise<number> {
  const leaf = ca.leaves.get("example.test");
  if (leaf === undefined) throw new Error("plain host has no leaf");
  const app = createHttpServer((_request, response) => {
    response.writeHead(200, { "content-type": "text/plain" });
    response.end("tunnelled");
  });
  const tls = createTlsServer({ key: leaf.key, cert: leaf.cert });
  tls.on("secureConnection", (socket) => app.emit("connection", socket));
  servers.push(tls);
  return listen(tls as never);
}

/**
 * A minimal Squid: accepts CONNECT and dials the mapped local port.
 *
 * The mapping is what lets the proxy keep using the real hostnames — and
 * therefore the real policy — while the sockets stay on loopback.
 */
async function startFakeSquid(
  routes: ReadonlyMap<string, number>,
  seen: string[],
  plainHttpPort: number,
): Promise<number> {
  const squid = createHttpServer((request, response) => {
    // Absolute-form cleartext, exactly as a client with HTTP_PROXY set would
    // send it. Squid resolves the host itself; the fake maps every allowed
    // cleartext host onto one loopback origin.
    const target = request.url ?? "";
    seen.push(target);
    if (!target.startsWith("http://example.test/")) {
      response.writeHead(403).end();
      return;
    }
    const upstream = httpRequest(
      {
        host: "127.0.0.1",
        port: plainHttpPort,
        method: request.method,
        path: new URL(target).pathname,
      },
      (upstreamResponse) => {
        response.writeHead(upstreamResponse.statusCode ?? 502, upstreamResponse.headers);
        upstreamResponse.pipe(response);
      },
    );
    upstream.on("error", () => response.writeHead(502).end());
    request.pipe(upstream);
  });
  squid.on("connect", (request, clientSocket: Socket, head: Buffer) => {
    const target = request.url ?? "";
    seen.push(target);
    const port = routes.get(target);
    if (port === undefined) {
      clientSocket.end("HTTP/1.1 403 Forbidden\r\n\r\n");
      return;
    }
    const upstream = netConnect({ host: "127.0.0.1", port }, () => {
      clientSocket.write("HTTP/1.1 200 Connection Established\r\n\r\n");
      if (head.length > 0) upstream.write(head);
      upstream.pipe(clientSocket);
      clientSocket.pipe(upstream);
    });
    upstream.on("error", () => clientSocket.destroy());
    clientSocket.on("error", () => upstream.destroy());
  });
  servers.push(squid);
  return listen(squid as never);
}

/** A cleartext origin behind the fake Squid, standing in for an http:// feed. */
async function startPlainHttpOrigin(): Promise<number> {
  const app = createHttpServer((_request, response) => {
    response.writeHead(200, { "content-type": "text/plain" });
    response.end("plain-http");
  });
  servers.push(app);
  return listen(app as never);
}

interface ClientResponse {
  readonly status: number;
  readonly body: string;
  readonly headers: Readonly<Record<string, string | string[] | undefined>>;
}

/** Issue a request through the proxy exactly as a client with proxy env would. */
function requestThroughProxy(
  proxyPort: number,
  host: string,
  path: string,
  options: {
    method?: string;
    headers?: Record<string, string>;
    ca: string;
  },
): Promise<ClientResponse> {
  return new Promise((resolve, reject) => {
    const socket = netConnect({ host: "127.0.0.1", port: proxyPort }, () => {
      socket.write(`CONNECT ${host}:443 HTTP/1.1\r\nHost: ${host}:443\r\n\r\n`);
    });

    let preamble = "";
    const onData = (chunk: Buffer): void => {
      preamble += chunk.toString("latin1");
      const end = preamble.indexOf("\r\n\r\n");
      if (end === -1) return;
      socket.removeListener("data", onData);

      const statusLine = preamble.slice(0, preamble.indexOf("\r\n"));
      if (!statusLine.includes("200")) {
        socket.destroy();
        reject(new Error(`proxy refused CONNECT: ${statusLine}`));
        return;
      }
      const leftover = preamble.slice(end + 4);
      if (leftover.length > 0) socket.unshift(Buffer.from(leftover, "latin1"));

      const secured = tlsConnect({ socket, servername: host, ca: options.ca }, () => {
        const headerLines = Object.entries(options.headers ?? {})
          .map(([name, value]) => `${name}: ${value}\r\n`)
          .join("");
        secured.write(
          `${options.method ?? "GET"} ${path} HTTP/1.1\r\nHost: ${host}\r\n` +
            `${headerLines}Connection: close\r\n\r\n`,
        );
      });

      let raw = "";
      secured.on("data", (chunk: Buffer) => {
        raw += chunk.toString("utf8");
      });
      secured.on("error", reject);
      secured.on("close", () => {
        const headerEnd = raw.indexOf("\r\n\r\n");
        const head = headerEnd === -1 ? raw : raw.slice(0, headerEnd);
        const body = headerEnd === -1 ? "" : raw.slice(headerEnd + 4);
        const headers: Record<string, string> = {};
        for (const line of head.split("\r\n").slice(1)) {
          const colon = line.indexOf(":");
          if (colon === -1) continue;
          headers[line.slice(0, colon).trim().toLowerCase()] = line.slice(colon + 1).trim();
        }
        resolve({
          status: Number(head.split("\r\n")[0]?.split(" ")[1] ?? 0),
          body,
          headers,
        });
      });
    };

    socket.on("data", onData);
    socket.on("error", reject);
  });
}

/** Issue an absolute-form cleartext request, as a client with HTTP_PROXY does. */
function plainHttpThroughProxy(proxyPort: number, target: string): Promise<ClientResponse> {
  return new Promise((resolve, reject) => {
    const request = httpRequest(
      { host: "127.0.0.1", port: proxyPort, method: "GET", path: target },
      (response) => {
        let body = "";
        response.on("data", (chunk: Buffer) => {
          body += chunk.toString("utf8");
        });
        response.on("end", () =>
          resolve({ status: response.statusCode ?? 0, body, headers: response.headers }),
        );
      },
    );
    request.on("error", reject);
    request.end();
  });
}

beforeAll(async () => {
  if (!hasOpenssl) return;
  workdir = mkdtempSync(join(tmpdir(), "ado-proxy-e2e-"));

  const upstreamCa = mintForTest(join(workdir, "upstream-ca"), ["dev.azure.com"]).materials;
  const plainCa = mintForTest(join(workdir, "plain-ca"), ["example.test"]).materials;
  const proxyCa = mintForTest(join(workdir, "proxy-ca"), POLICY.protected_hosts).materials;

  const upstreamCalls: UpstreamCall[] = [];
  const tunnelTargets: string[] = [];
  const adoPort = await startFakeAdo(upstreamCa, upstreamCalls);
  const plainPort = await startPlainHost(plainCa);
  const plainHttpPort = await startPlainHttpOrigin();
  const squidPort = await startFakeSquid(
    new Map([
      ["dev.azure.com:443", adoPort],
      ["example.test:443", plainPort],
    ]),
    tunnelTargets,
    plainHttpPort,
  );

  const tokenFile = join(workdir, "token");
  writeFileSync(tokenFile, `${CANARY}\n`, { mode: 0o600 });

  const config: ProxyConfig = {
    listenAddress: "127.0.0.1",
    listenPort: 0,
    upstreamProxy: `http://127.0.0.1:${squidPort}`,
    tokenFile,
    publicCaFile: join(workdir, "ca.pem"),
    policy: POLICY,
  };

  const server = createProxyServer({
    config,
    ca: proxyCa,
    tokens: new TokenSource(tokenFile),
    log: new DecisionLog(join(workdir, "decisions")),
    upstreamCa: upstreamCa.caCertPem,
  });
  servers.push(server);
  const proxyPort = await listen(server as never);

  harness = {
    proxyPort,
    proxyCaPem: proxyCa.caCertPem,
    upstreamCalls,
    tunnelTargets,
    tokenFile,
  };

  // Keep the plain CA reachable for the tunnel assertion.
  plainCaPem = plainCa.caCertPem;
});

let plainCaPem = "";

afterAll(async () => {
  await Promise.all(
    servers.map(
      (server) =>
        new Promise<void>((resolve) => {
          // Tunnelled sockets stay open by design, and the intercepting inner
          // servers hold their own. Ask politely, then stop waiting — teardown
          // is not what these tests are proving.
          (server as { closeAllConnections?: () => void }).closeAllConnections?.();
          server.close(() => resolve());
          setTimeout(resolve, 500).unref();
        }),
    ),
  );
  if (workdir !== undefined) rmSync(workdir, { recursive: true, force: true });
});

const suite = hasOpenssl ? describe : describe.skip;

suite("ado-proxy end to end", () => {
  it("injects the bearer only on an allowed read, and strips the client's", async () => {
    const before = harness.upstreamCalls.length;
    const response = await requestThroughProxy(
      harness.proxyPort,
      "dev.azure.com",
      `/${ORGANIZATION}/_apis/projects?api-version=7.1&stateFilter=all&$top=1&$skip=0`,
      {
        ca: harness.proxyCaPem,
        headers: {
          // What `az devops` sends once AZURE_DEVOPS_EXT_PAT is set.
          Authorization: `Basic ${Buffer.from(`:${SENTINEL}`).toString("base64")}`,
          Accept: "application/json;api-version=7.1",
        },
      },
    );

    expect(response.status).toBe(200);
    const call = harness.upstreamCalls[before];
    expect(call).toBeDefined();
    expect(call?.authorization).toBe(`Bearer ${CANARY}`);
    // The sentinel must not survive in any form.
    expect(call?.authorization).not.toContain(SENTINEL);
    expect(call?.headerNames).not.toContain("cookie");
  });

  it("filters the response and never returns upstream session material", async () => {
    const response = await requestThroughProxy(
      harness.proxyPort,
      "dev.azure.com",
      `/${ORGANIZATION}/_apis/projects?api-version=7.1&stateFilter=all&$top=1&$skip=0`,
      { ca: harness.proxyCaPem },
    );
    const body = JSON.parse(response.body) as { count: number; value: { name: string }[] };
    // The upstream returned two projects; the agent may only learn about one.
    expect(body.count).toBe(1);
    expect(body.value[0]?.name).toBe("Widgets");
    expect(response.headers["set-cookie"]).toBeUndefined();
    expect(response.body).not.toContain(CANARY);
  });

  it("refuses a write without contacting the upstream", async () => {
    const before = harness.upstreamCalls.length;
    const response = await requestThroughProxy(
      harness.proxyPort,
      "dev.azure.com",
      `/${ORGANIZATION}/_apis/wit/workitems/$Bug?api-version=7.1`,
      { ca: harness.proxyCaPem, method: "POST" },
    );
    expect(response.status).toBe(403);
    // The credential must never be exercised on a request that was denied.
    expect(harness.upstreamCalls.length).toBe(before);
    expect(response.body).not.toContain(CANARY);
  });

  it("refuses a cross-project read without contacting the upstream", async () => {
    const before = harness.upstreamCalls.length;
    const response = await requestThroughProxy(
      harness.proxyPort,
      "dev.azure.com",
      `/${ORGANIZATION}/_apis/projects/Secrets?api-version=7.1`,
      { ca: harness.proxyCaPem },
    );
    expect(response.status).toBe(403);
    expect(harness.upstreamCalls.length).toBe(before);
  });

  it("refuses an uncatalogued route without contacting the upstream", async () => {
    const before = harness.upstreamCalls.length;
    const response = await requestThroughProxy(
      harness.proxyPort,
      "dev.azure.com",
      `/${ORGANIZATION}/_apis/serviceendpoint/endpoints?api-version=7.1`,
      { ca: harness.proxyCaPem },
    );
    expect(response.status).toBe(403);
    expect(harness.upstreamCalls.length).toBe(before);
  });

  it("byte-tunnels a non-protected host end to end", async () => {
    const before = harness.upstreamCalls.length;
    const response = await requestThroughProxy(
      harness.proxyPort,
      "example.test",
      "/anything",
      // Trusting only the plain host's own CA proves the proxy did not
      // terminate this connection: an intercepted one would present the
      // proxy's certificate and fail verification here.
      { ca: plainCaPem },
    );
    expect(response.status).toBe(200);
    // The tunnelled response is transfer-encoded; the point is that the body
    // arrived intact from the origin, not its framing.
    expect(response.body).toContain("tunnelled");
    expect(harness.tunnelTargets).toContain("example.test:443");
    expect(harness.upstreamCalls.length).toBe(before);
  });

  it("answers the readiness probe without revealing policy detail", async () => {
    // AWF polls this before starting the agent so the agent cannot race a
    // proxy that has not finished minting its CA.
    const response = await plainHttpThroughProxy(harness.proxyPort, HEALTH_PATH);
    expect(response.status).toBe(200);
    expect(JSON.parse(response.body)).toEqual({ status: "ok" });
  });

  it("refuses any other origin-form request rather than acting as a relay", async () => {
    const response = await plainHttpThroughProxy(harness.proxyPort, "/anything");
    expect(response.status).toBe(403);
  });

  it("relays plain HTTP for a non-protected host so http:// sources keep working", async () => {
    // The agent's HTTP_PROXY points here, so refusing cleartext would silently
    // break any http:// package source.
    const response = await plainHttpThroughProxy(
      harness.proxyPort,
      `http://example.test/plain`,
    );
    expect(response.status).toBe(200);
    expect(response.body).toContain("plain-http");
  });

  it("refuses cleartext to a protected host", async () => {
    const before = harness.upstreamCalls.length;
    const response = await plainHttpThroughProxy(
      harness.proxyPort,
      `http://dev.azure.com/${ORGANIZATION}/_apis/projects/Widgets?api-version=7.1`,
    );
    // Relaying this would put the policy path — and the bearer — on an
    // unencrypted hop.
    expect(response.status).toBe(403);
    expect(response.body).toContain("HTTPS");
    expect(harness.upstreamCalls.length).toBe(before);
  });

  it("returns denials in the WrappedException shape clients can surface", async () => {
    const response = await requestThroughProxy(
      harness.proxyPort,
      "dev.azure.com",
      `/${ORGANIZATION}/_apis/projects/Secrets?api-version=7.1`,
      { ca: harness.proxyCaPem },
    );
    const body = JSON.parse(response.body) as { message: string; typeKey: string };
    // `az` and every msrest-based SDK read `message`; without this shape a
    // denial surfaces as "unexpected response" with no actionable detail.
    expect(body.typeKey).toBe("out-of-scope");
    expect(body.message).toContain("ado-proxy");
    // No header that would send a client into a retry or an interactive login.
    expect(response.headers["www-authenticate"]).toBeUndefined();
    expect(response.headers["retry-after"]).toBeUndefined();
    expect(response.headers.location).toBeUndefined();
  });

  it("fails closed, not unauthenticated, when the credential is missing", async () => {
    writeFileSync(harness.tokenFile, "   \n", { mode: 0o600 });
    const before = harness.upstreamCalls.length;
    try {
      const response = await requestThroughProxy(
        harness.proxyPort,
        "dev.azure.com",
        `/${ORGANIZATION}/_apis/projects/Widgets?api-version=7.1`,
        { ca: harness.proxyCaPem },
      );
      // 502 rather than 401/429/503: msrest retries those, which would turn one
      // failure into several upstream calls.
      expect(response.status).toBe(502);
      expect(harness.upstreamCalls.length).toBe(before);
    } finally {
      writeFileSync(harness.tokenFile, `${CANARY}\n`, { mode: 0o600 });
    }
  });

  it("picks up a rotated token without a restart", async () => {
    const rotated = "rotated-bearer-1a2b3c4d";
    writeFileSync(harness.tokenFile, `${rotated}\n`, { mode: 0o600 });
    try {
      const before = harness.upstreamCalls.length;
      await requestThroughProxy(
        harness.proxyPort,
        "dev.azure.com",
        `/${ORGANIZATION}/_apis/projects/Widgets?api-version=7.1`,
        { ca: harness.proxyCaPem },
      );
      expect(harness.upstreamCalls[before]?.authorization).toBe(`Bearer ${rotated}`);
    } finally {
      writeFileSync(harness.tokenFile, `${CANARY}\n`, { mode: 0o600 });
    }
  });
});
