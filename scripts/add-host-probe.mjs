/**
 * Spike: can `--add-host` redirect a container's Azure DevOps traffic to the
 * policy proxy, with TLS verification left ON?
 *
 * This is the mechanism the ADO MCP path depends on. `@azure-devops/mcp`
 * hardcodes `"https://dev.azure.com/" + orgName` with no override, and 8 of its
 * HTTP call sites use raw `fetch()`, which ignores proxy environment variables
 * (undici honours them only under `NODE_USE_ENV_PROXY`, which needs Node ≥24.5
 * against a pinned `node:20-slim`). So redirection has to happen *below* the
 * application, without the client cooperating.
 *
 * `--add-host` writes `/etc/hosts` directly, so it needs no DNS at all. That
 * matters because AWF itself pre-registers `/etc/hosts` entries precisely
 * because Docker's embedded DNS is unreachable under gVisor and on ARC/DinD —
 * a Docker network alias would be fragile in exactly those environments.
 *
 * What this proves, or fails to:
 *
 *   1. a client asking for `https://dev.azure.com/...` reaches our server;
 *   2. it does so with `rejectUnauthorized` left ON, trusting only the CA we
 *      supply via `NODE_EXTRA_CA_CERTS` — i.e. interception is *trusted*, not
 *      bypassed;
 *   3. **both** Node HTTP paths work: `https.get` (typed-rest-client's path)
 *      and global `fetch` (undici — the one that ignores proxies);
 *   4. an unrelated host is *not* redirected, so the mechanism is narrow.
 *
 * Usage: node scripts/add-host-probe.mjs
 */
import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const DOCKER_DIR = "C:\\Users\\devinejames\\AppData\\Local\\Programs\\DockerDesktop\\resources\\bin";
const NETWORK = "ado-proxy-spike-net";
const SERVER = "ado-proxy-spike-server";
const IMAGE = "node:20-slim";
const CANARY = "reached-the-policy-proxy-9f3a";

/** Resolve `docker`, which is not on the default shell PATH on this machine. */
function docker(args, options = {}) {
  return execFileSync(join(DOCKER_DIR, "docker.exe"), args, {
    encoding: "utf8",
    timeout: 180_000,
    ...options,
  });
}

function quietDocker(args) {
  try {
    return docker(args, { stdio: ["ignore", "pipe", "ignore"] });
  } catch {
    return "";
  }
}

/** Git for Windows ships openssl but does not export it. */
function ensureOpenssl() {
  for (const dir of ["C:\\Program Files\\Git\\usr\\bin", "C:\\Program Files\\Git\\mingw64\\bin"]) {
    try {
      execFileSync("openssl", ["version"], { stdio: "ignore" });
      return;
    } catch {
      process.env.PATH = `${dir};${process.env.PATH ?? ""}`;
    }
  }
  execFileSync("openssl", ["version"], { stdio: "ignore" });
}

const work = mkdtempSync(join(tmpdir(), "add-host-spike-"));
let cleanupNeeded = true;

function cleanup() {
  if (!cleanupNeeded) return;
  cleanupNeeded = false;
  quietDocker(["rm", "-f", SERVER]);
  quietDocker(["network", "rm", NETWORK]);
  rmSync(work, { recursive: true, force: true });
}

process.on("exit", cleanup);

try {
  ensureOpenssl();

  // ── certificate for dev.azure.com ──────────────────────────────────────
  // The leaf carries a SAN for the real hostname: the client must be able to
  // verify it *as* dev.azure.com, which is the whole point of interception.
  execFileSync("openssl", [
    "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "2",
    "-subj", "/CN=ado-proxy spike CA", "-keyout", "ca.key", "-out", "ca.pem",
    "-addext", "basicConstraints=critical,CA:TRUE,pathlen:0",
  ], { cwd: work, stdio: ["ignore", "ignore", "pipe"] });

  writeFileSync(join(work, "leaf.ext"),
    "basicConstraints=CA:FALSE\n" +
    "keyUsage=critical,digitalSignature,keyEncipherment\n" +
    "extendedKeyUsage=serverAuth\n" +
    "subjectAltName=DNS:dev.azure.com\n");

  execFileSync("openssl", [
    "req", "-new", "-newkey", "rsa:2048", "-nodes", "-subj", "/CN=dev.azure.com",
    "-keyout", "leaf.key", "-out", "leaf.csr",
  ], { cwd: work, stdio: ["ignore", "ignore", "pipe"] });

  execFileSync("openssl", [
    "x509", "-req", "-in", "leaf.csr", "-CA", "ca.pem", "-CAkey", "ca.key",
    "-CAcreateserial", "-days", "2", "-extfile", "leaf.ext", "-out", "leaf.pem",
  ], { cwd: work, stdio: ["ignore", "ignore", "pipe"] });

  // ── the stand-in policy proxy ──────────────────────────────────────────
  writeFileSync(join(work, "server.mjs"), `
import { createServer } from "node:https";
import { readFileSync } from "node:fs";
const opts = { key: readFileSync("/certs/leaf.key"), cert: readFileSync("/certs/leaf.pem") };
createServer(opts, (req, res) => {
  console.log("SERVER_SAW " + req.method + " " + req.url + " host=" + req.headers.host);
  const body = JSON.stringify({ marker: ${JSON.stringify(CANARY)}, url: req.url });
  res.writeHead(200, { "content-type": "application/json" });
  res.end(body);
}).listen(443, "0.0.0.0", () => console.log("READY"));
`);

  // ── the client: knows nothing about proxies ────────────────────────────
  // Deliberately requests the real public hostname with verification ON.
  writeFileSync(join(work, "client.mjs"), `
import { get } from "node:https";

function viaHttpsGet(url) {
  return new Promise((resolve, reject) => {
    get(url, (res) => {
      let body = "";
      res.on("data", (c) => (body += c));
      res.on("end", () => resolve(body));
    }).on("error", reject);
  });
}

const results = {};
// Path 1: node:https — what azure-devops-node-api / typed-rest-client uses.
try {
  results.httpsGet = await viaHttpsGet("https://dev.azure.com/myorg/_apis");
} catch (e) { results.httpsGet = "ERROR: " + e.message; }

// Path 2: global fetch (undici) — the path that ignores proxy env vars, and
// therefore the one that must be redirected below the application.
try {
  const r = await fetch("https://dev.azure.com/myorg/_apis/projects");
  results.fetch = await r.text();
} catch (e) { results.fetch = "ERROR: " + e.message; }

// Control: an unrelated host must NOT be redirected.
try {
  await viaHttpsGet("https://example.invalid/");
  results.control = "UNEXPECTEDLY RESOLVED";
} catch (e) { results.control = "ERROR: " + e.code || e.message; }

console.log(JSON.stringify(results, null, 2));
`);

  // ── run it ─────────────────────────────────────────────────────────────
  quietDocker(["rm", "-f", SERVER]);
  quietDocker(["network", "rm", NETWORK]);
  docker(["network", "create", NETWORK], { stdio: ["ignore", "pipe", "pipe"] });

  docker([
    "run", "-d", "--name", SERVER, "--network", NETWORK,
    "-v", `${work}:/certs:ro`,
    IMAGE, "node", "/certs/server.mjs",
  ], { stdio: ["ignore", "pipe", "pipe"] });

  // Wait for the listener rather than sleeping blindly.
  let ready = false;
  for (let i = 0; i < 30; i += 1) {
    if (quietDocker(["logs", SERVER]).includes("READY")) { ready = true; break; }
    execFileSync(process.execPath, ["-e", "setTimeout(()=>{},500)"]);
  }
  if (!ready) {
    console.log("server did not become ready; logs:");
    console.log(quietDocker(["logs", SERVER]));
    process.exit(1);
  }

  const serverIp = docker([
    "inspect", "-f", "{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}", SERVER,
  ], { stdio: ["ignore", "pipe", "pipe"] }).trim();

  console.log(`server container IP: ${serverIp}`);
  console.log(`running client with --add-host dev.azure.com:${serverIp}\n`);

  const clientOut = docker([
    "run", "--rm", "--network", NETWORK,
    "--add-host", `dev.azure.com:${serverIp}`,
    "-v", `${work}:/certs:ro`,
    "-e", "NODE_EXTRA_CA_CERTS=/certs/ca.pem",
    IMAGE, "node", "/certs/client.mjs",
  ], { stdio: ["ignore", "pipe", "pipe"] });

  console.log("=== client results ===");
  console.log(clientOut);
  console.log("=== server observed ===");
  const serverLog = quietDocker(["logs", SERVER]);
  for (const line of serverLog.split("\n").filter((l) => l.includes("SERVER_SAW"))) {
    console.log(`  ${line.trim()}`);
  }

  // ── verdict ────────────────────────────────────────────────────────────
  const parsed = JSON.parse(clientOut);
  const httpsOk = String(parsed.httpsGet).includes(CANARY);
  const fetchOk = String(parsed.fetch).includes(CANARY);
  const controlOk = String(parsed.control).startsWith("ERROR");

  console.log("\n=== verdict ===");
  console.log(`  node:https redirected, TLS verified : ${httpsOk}`);
  console.log(`  global fetch redirected, TLS verified: ${fetchOk}`);
  console.log(`  unrelated host NOT redirected        : ${controlOk}`);
  console.log(`  OVERALL: ${httpsOk && fetchOk && controlOk ? "PASS" : "FAIL"}`);
  process.exitCode = httpsOk && fetchOk && controlOk ? 0 : 1;
} finally {
  cleanup();
}
