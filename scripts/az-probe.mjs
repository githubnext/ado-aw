/**
 * Live Azure CLI probe against the real `ado-proxy` bundle.
 *
 * Not a unit test and not part of any suite: it drives the *actual* `az`
 * binary through the *actual* bundle to answer a question the mocked E2E
 * cannot — does stock tooling work through this proxy, and does the catalog
 * match the requests `az` really makes?
 *
 * Topology (no DNS changes, no real Azure DevOps, no real credential):
 *
 *   az --HTTPS_PROXY--> ado-proxy --CONNECT--> fake Squid --> fake ADO
 *
 * `az` is given the proxy's public CA via REQUESTS_CA_BUNDLE, so TLS
 * verification stays ON throughout — this proves interception is *trusted*,
 * not bypassed. The bearer the proxy injects is a canary string; the fake
 * upstream records every request so we can diff what `az` asked for against
 * what the catalog allows.
 *
 * Usage: node scripts/az-probe.mjs
 */
import { execFileSync, spawn } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer as createHttpServer } from "node:http";
import { connect as netConnect } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { createServer as createTlsServer } from "node:tls";

const here = dirname(fileURLToPath(import.meta.url));
const bundle = join(here, "ado-script", "ado-proxy.js");

const CANARY = "canary-bearer-probe-9f3a2c";
const ORG = "contoso";
const PROJECT = "Widgets";
const REPO = "widget-api";

// Git for Windows ships openssl but does not export it.
for (const dir of ["C:\\Program Files\\Git\\usr\\bin", "C:\\Program Files\\Git\\mingw64\\bin"]) {
  try {
    execFileSync("openssl", ["version"], { stdio: "ignore" });
    break;
  } catch {
    process.env.PATH = `${dir};${process.env.PATH ?? ""}`;
  }
}

const work = mkdtempSync(join(tmpdir(), "az-probe-"));
const servers = [];
/** Every request the fake Azure DevOps upstream actually received. */
const upstreamCalls = [];
/** Everything the proxy allowed or denied, parsed from its decision log. */
const proxyDecisions = [];

function listen(server, port = 0) {
  return new Promise((resolve) => {
    server.listen(port, "127.0.0.1", () => resolve(server.address().port));
  });
}

function mintCa(dir, hosts) {
  const list = Array.isArray(hosts) ? hosts : [hosts];
  const primary = list[0];
  execFileSync(
    "openssl",
    ["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "2", "-subj", `/CN=${primary} CA`,
      "-keyout", "ca.key", "-out", "ca.pem",
      "-addext", "basicConstraints=critical,CA:TRUE,pathlen:0"],
    { cwd: dir, stdio: ["ignore", "ignore", "pipe"] },
  );
  const san = list.map((host) => `DNS:${host}`).join(",");
  writeFileSync(join(dir, "leaf.ext"),
    `basicConstraints=CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=${san}\n`);
  execFileSync("openssl", ["req", "-new", "-newkey", "rsa:2048", "-nodes", "-subj", `/CN=${primary}`,
    "-keyout", "leaf.key", "-out", "leaf.csr"], { cwd: dir, stdio: ["ignore", "ignore", "pipe"] });
  execFileSync("openssl", ["x509", "-req", "-in", "leaf.csr", "-CA", "ca.pem", "-CAkey", "ca.key",
    "-CAcreateserial", "-days", "2", "-extfile", "leaf.ext", "-out", "leaf.pem"],
    { cwd: dir, stdio: ["ignore", "ignore", "pipe"] });
  return {
    key: readText(join(dir, "leaf.key")),
    cert: readText(join(dir, "leaf.pem")),
    ca: readText(join(dir, "ca.pem")),
  };
}

function readText(path) {
  return execFileSync(process.execPath, ["-e", `process.stdout.write(require('node:fs').readFileSync(${JSON.stringify(path)},'utf8'))`], { encoding: "utf8" });
}

/**
 * The `OPTIONS /_apis` discovery document.
 *
 * Host-aware, because the two protected hosts serve different things and
 * conflating them makes `az` address the wrong one:
 *
 *   - **SPS** (`app.vssps.visualstudio.com`) is a deployment-level service. It
 *     advertises the *location* service, which is how `az` discovers which host
 *     owns an area. Advertising org-level resources here would make `az` ask
 *     SPS for `/_apis/projects`, which real Azure DevOps never serves — and
 *     would tempt us into widening the catalog to fit a harness bug.
 *   - **Organization host** (`dev.azure.com`) advertises the org-level
 *     resources `az` actually reads.
 */
function resourceLocations(host) {
  const entry = (id, area, resourceName, routeTemplate) => ({
    id,
    area,
    resourceName,
    routeTemplate,
    resourceVersion: 1,
    minVersion: "1.0",
    maxVersion: "7.2",
    releasedVersion: "7.1",
  });

  if (host.startsWith("app.vssps")) {
    const value = [
      entry("e81700f7-3be2-46de-8624-2eb35882fcaa", "location", "resourceAreas", "_apis/{resource}/{areaId}"),
    ];
    return { count: value.length, value };
  }

  const value = [
    entry("603fe2ac-9723-48b9-88ad-09305aa6c6e1", "core", "projects", "_apis/{resource}/{*projectId}"),
    // The `location` area is required: without it `az` fails outright with
    // "API resource location e81700f7-… is not registered".
    entry("e81700f7-3be2-46de-8624-2eb35882fcaa", "location", "resourceAreas", "_apis/{resource}/{areaId}"),
    entry("225f7195-f9c7-4d14-ab28-a83f7ff77e1f", "git", "repositories", "{project}/_apis/git/{resource}/{repositoryId}"),
    entry("dbeaf647-6167-421a-bda9-c9327b25e2e6", "build", "builds", "{project}/_apis/build/{resource}/{buildId}"),
  ];
  return { count: value.length, value };
}

/**
 * The upstream's own resource-area list.
 *
 * Deliberately points at hosts *other* than the intercepted one, so the run
 * proves the proxy rewrites them rather than the fake upstream having been
 * pre-cooked to look correct.
 */
function upstreamResourceAreas() {
  const upstream = `https://vsrm.dev.azure.com/${ORG}/`;
  return [
    { id: "79134c72-4a58-4b42-976c-04e7115f32bf", name: "core", locationUrl: upstream },
    { id: "4e080c62-fa21-4fbc-8fef-2a10a2b38049", name: "git", locationUrl: upstream },
    { id: "5d6898bb-45ec-463f-95f9-54d49c71752e", name: "build", locationUrl: upstream },
    { id: "5264459e-e5e0-4bd8-b118-0985e68a4ec5", name: "wit", locationUrl: upstream },
    { id: "e81700f7-3be2-46de-8624-2eb35882fcaa", name: "location", locationUrl: upstream },
  ];
}

/** Realistic Azure DevOps responses for the routes az actually calls. */
function respond(url, method, host, response) {
  const path = url.split("?")[0].toLowerCase();
  const json = (body) => {
    const text = JSON.stringify(body);
    response.writeHead(200, { "content-type": "application/json", "content-length": Buffer.byteLength(text) });
    response.end(text);
  };

  if (method === "OPTIONS") return json(resourceLocations(host));

  // The location service. The upstream advertises non-intercepted hosts; the
  // proxy's rewrite is what must bring them back to the policed origin.
  if (path.includes("/_apis/resourceareas")) {
    if (path.endsWith("/resourceareas")) {
      const areas = upstreamResourceAreas();
      return json({ count: areas.length, value: areas });
    }
    return json({ id: path.split("/").pop(), name: "git", locationUrl: `https://vsrm.dev.azure.com/${ORG}/` });
  }

  if (path.endsWith("/_apis/projects")) {
    return json({
      count: 2,
      value: [
        { id: "11111111-1111-1111-1111-111111111111", name: PROJECT, state: "wellFormed", visibility: "private" },
        { id: "33333333-3333-3333-3333-333333333333", name: "Secrets", state: "wellFormed", visibility: "private" },
      ],
    });
  }
  if (path.includes("/_apis/connectiondata")) {
    return json({ authenticatedUser: { id: "0", providerDisplayName: "probe" }, instanceId: "x", deploymentId: "y" });
  }
  if (path.includes(`/_apis/git/repositories/${REPO.toLowerCase()}`)) {
    return json({
      id: "22222222-2222-2222-2222-222222222222",
      name: REPO,
      project: { id: "11111111-1111-1111-1111-111111111111", name: PROJECT },
      defaultBranch: "refs/heads/main",
    });
  }
  if (path.includes("/_apis/build/builds")) {
    return json({ count: 0, value: [] });
  }
  return json({ count: 0, value: [] });
}

async function start() {
  // --- fake Azure DevOps -------------------------------------------------
  const adoDir = join(work, "ado");
  execFileSync(process.execPath, ["-e", `require('node:fs').mkdirSync(${JSON.stringify(adoDir)},{recursive:true})`]);
  const adoCert = mintCa(adoDir, ["dev.azure.com", "app.vssps.visualstudio.com"]);

  const adoApp = createHttpServer((request, response) => {
    const host = String(request.headers.host ?? "").split(":")[0].toLowerCase();
    upstreamCalls.push({
      method: request.method,
      url: request.url,
      host,
      authorization: request.headers.authorization ?? "(none)",
      accept: request.headers.accept ?? "(none)",
    });
    respond(request.url, request.method, host, response);
  });
  const adoTls = createTlsServer({ key: adoCert.key, cert: adoCert.cert });
  adoTls.on("secureConnection", (socket) => adoApp.emit("connection", socket));
  servers.push(adoTls);
  const adoPort = await listen(adoTls);

  // --- fake Squid: the proxy's only route out ----------------------------
  const squid = createHttpServer((_request, response) => response.writeHead(405).end());
  squid.on("connection", (socket) => {
    console.log(`  [squid] TCP connection from ${socket.remoteAddress}:${socket.remotePort}`);
  });
  squid.on("connect", (request, clientSocket, head) => {
    console.log(`  [squid] CONNECT ${request.url}`);
    // Both protected hosts resolve to the one fake upstream, whose certificate
    // carries a SAN for each.
    if (!["dev.azure.com:443", "app.vssps.visualstudio.com:443"].includes(request.url)) {
      clientSocket.end("HTTP/1.1 403 Forbidden\r\n\r\n");
      return;
    }
    const upstream = netConnect({ host: "127.0.0.1", port: adoPort }, () => {
      clientSocket.write("HTTP/1.1 200 Connection Established\r\n\r\n");
      if (head.length > 0) upstream.write(head);
      upstream.pipe(clientSocket);
      clientSocket.pipe(upstream);
    });
    upstream.on("error", (error) => {
      console.log(`  [squid] upstream error: ${error.message}`);
      clientSocket.destroy();
    });
    clientSocket.on("error", () => upstream.destroy());
  });
  squid.on("clientError", (error) => console.log(`  [squid] clientError: ${error.message}`));
  servers.push(squid);
  const squidPort = await listen(squid);

  // --- the real ado-proxy bundle ----------------------------------------
  const policy = {
    catalog_version: "ado-aw/ado-proxy-catalog/v1",
    organization: ORG,
    project: PROJECT,
    project_id: "11111111-1111-1111-1111-111111111111",
    repository: REPO,
    repository_id: "22222222-2222-2222-2222-222222222222",
    capabilities: ["discovery", "core", "repos", "pipelines", "boards"],
    protected_hosts: ["dev.azure.com", "app.vssps.visualstudio.com"],
    allowed_resource_areas: ["79134c72-4a58-4b42-976c-04e7115f32bf"],
  };
  writeFileSync(join(work, "policy.json"), JSON.stringify(policy));
  writeFileSync(join(work, "token"), CANARY);
  const caOut = join(work, "proxy-ca.pem");
  writeFileSync(caOut, "");

  const proxyPort = 18080;
  console.log(`  [harness] squid=${squidPort} fakeAdo=${adoPort} proxy=${proxyPort}`);
  const adoCaFile = join(work, "fake-ado-ca.pem");
  writeFileSync(adoCaFile, adoCert.ca);
  const proxy = spawn(process.execPath, [bundle,
    "--policy-file", join(work, "policy.json"),
    "--token-file", join(work, "token"),
    "--public-ca-file", caOut,
    "--upstream-proxy", `http://127.0.0.1:${squidPort}`,
    "--listen-address", "127.0.0.1",
    "--listen-port", String(proxyPort),
    "--log-dir", join(work, "log"),
  ], {
    stdio: ["ignore", "pipe", "pipe"],
    env: {
      ...process.env,
      // The proxy verifies the *upstream* certificate and correctly refuses a
      // self-signed one ("unable to verify the first certificate"). Trust the
      // harness's fake-ADO CA so the probe can proceed — verification stays on,
      // this only adds one CA. Nothing in the bundle disables it.
      NODE_EXTRA_CA_CERTS: adoCaFile,
    },
  });

  proxy.stderr.on("data", (chunk) => process.stdout.write(`  [proxy] ${chunk}`));
  await new Promise((resolve) => setTimeout(resolve, 2500));

  return { proxyPort, caOut, proxy };
}

/**
 * Resolve the `az` entry point.
 *
 * On Windows `az` is a `.cmd` shim, which `execFileSync` cannot spawn
 * directly. Prefer the Python entry point when we can find it so the child is
 * a real executable rather than a shell.
 */
function resolveAz() {
  const command = process.platform === "win32" ? "where" : "which";
  try {
    const found = execFileSync(command, ["az"], { encoding: "utf8" })
      .split(/\r?\n/)
      .map((line) => line.trim())
      .filter(Boolean);
    const cmd = found.find((path) => path.toLowerCase().endsWith(".cmd")) ?? found[0];
    return cmd;
  } catch {
    return "az";
  }
}

const AZ = resolveAz();

/**
 * Run `az` **asynchronously**.
 *
 * This must not be `execFileSync`. The fake Squid and fake Azure DevOps servers
 * live in *this* process, so a synchronous child blocks the event loop and they
 * can never accept a connection — the proxy then reports "timed out opening a
 * tunnel" and nothing reaches the upstream. The proxy itself is a separate
 * process, which is why `az -> proxy` worked while `proxy -> squid` did not.
 */
function runAz(args, env) {
  const useShell = process.platform === "win32";
  // With `shell: true` the command and args are joined into one shell string,
  // so a path containing spaces ("C:\Program Files\...") must be quoted.
  const command = useShell ? `"${AZ}"` : AZ;
  return new Promise((resolve) => {
    const child = spawn(command, args, { env, shell: useShell });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
    });
    const timer = setTimeout(() => child.kill(), 180_000);
    child.on("close", (code) => {
      clearTimeout(timer);
      resolve({ ok: code === 0, code, stdout, stderr });
    });
    child.on("error", (error) => {
      clearTimeout(timer);
      resolve({ ok: false, code: -1, stdout, stderr: String(error) });
    });
  });
}

const { proxyPort, caOut, proxy } = await start();

const azEnv = {
  ...process.env,
  HTTPS_PROXY: `http://127.0.0.1:${proxyPort}`,
  HTTP_PROXY: `http://127.0.0.1:${proxyPort}`,
  // TLS verification stays ON — az must *trust* the interception CA.
  REQUESTS_CA_BUNDLE: caOut,
  // What an author would have set today; the proxy must strip it.
  AZURE_DEVOPS_EXT_PAT: "sentinel-pat-must-not-reach-upstream",
  AZURE_CORE_COLLECT_TELEMETRY: "no",
  AZURE_CORE_ONLY_SHOW_ERRORS: "true",
};

const scenarios = [
  ["project list (in scope)", ["devops", "project", "list", "--organization", `https://dev.azure.com/${ORG}`, "-o", "json"]],
  ["repo show (in scope)", ["repos", "show", "--repository", REPO, "--organization", `https://dev.azure.com/${ORG}`, "--project", PROJECT, "-o", "json", "--debug"]],
];

for (const [label, args] of scenarios) {
  console.log(`\n=== az ${label} ===`);
  const before = upstreamCalls.length;
  const result = await runAz(args, azEnv);
  console.log(`  exit code: ${result.code}`);
  // Full stderr goes to the OS temp dir, not the repo — these are debug
  // artifacts of a probe run, not source.
  writeFileSync(join(tmpdir(), `az-probe-${label.split(" ")[0]}-stderr.log`), result.stderr ?? "");
  if (!result.ok) console.log(`  stderr: ${(result.stderr || "(empty)").split("\n").filter((l) => l.includes("ERROR")).slice(0, 4).join("\n          ")}`);
  else console.log(`  stdout: ${result.stdout.slice(0, 300).replace(/\s+/g, " ")}`);
  console.log(`  upstream requests: ${upstreamCalls.length - before}`);
}

console.log("\n================ REQUESTS THAT REACHED THE FAKE ADO ================");
for (const call of upstreamCalls) {
  console.log(`  ${call.method} ${call.url}`);
  console.log(`      auth:   ${call.authorization}`);
  console.log(`      accept: ${call.accept}`);
}

console.log("\n================ PROXY DECISION LOG ================");
try {
  const log = readText(join(work, "log", "ado-proxy-decisions.jsonl"));
  for (const line of log.split("\n").filter(Boolean)) {
    const record = JSON.parse(line);
    if (record.schema) continue;
    proxyDecisions.push(record);
    console.log(`  ${record.decision.toUpperCase().padEnd(5)} ${record.method} ${record.operation ?? record.reason ?? ""} ${record.detail ? `— ${record.detail}` : ""}`);
  }
} catch (error) {
  console.log(`  (no decision log: ${error.message})`);
}

console.log("\n================ CANARY CHECK ================");
const leaked = upstreamCalls.some((call) => call.authorization.includes("sentinel-pat"));
const injected = upstreamCalls.some((call) => call.authorization === `Bearer ${CANARY}`);
console.log(`  sentinel PAT reached upstream: ${leaked}   (must be false)`);
console.log(`  proxy bearer reached upstream: ${injected}  (must be true if anything was allowed)`);

proxy.kill();
for (const server of servers) server.close();
setTimeout(() => {
  rmSync(work, { recursive: true, force: true });
  process.exit(0);
}, 500);
