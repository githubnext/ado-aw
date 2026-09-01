/**
 * Spike: can the Azure CLI be kept entirely on a policy endpoint, or does it
 * always reach the deployment-level SPS host?
 *
 * An earlier probe saw `az` contact `app.vssps.visualstudio.com` even when
 * `--organization` pointed elsewhere. That probe served a deliberately minimal
 * resource-location document, so the question is whether `az` was *falling
 * back* to SPS because the document did not tell it where areas live — or
 * whether some calls are hardcoded to SPS regardless.
 *
 * This matters because the discovery document is compiler-controlled in
 * production: if a faithful one keeps `az` local, SPS never needs allowing.
 *
 * Method: serve the same endpoint twice, once with a document that maps the
 * `location` area back to our own endpoint and once without, and compare which
 * hosts `az` resolves. DNS resolution is intercepted in-process so a stray SPS
 * lookup is *observed* rather than silently escaping to the real internet.
 *
 * Usage: node scripts/sps-probe.mjs
 */
import { execFileSync, spawn } from "node:child_process";
import dns from "node:dns";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:https";
import { tmpdir } from "node:os";
import { join } from "node:path";

const ORG = "myorg";

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

function resolveAz() {
  const found = execFileSync("where", ["az"], { encoding: "utf8" })
    .split(/\r?\n/).map((l) => l.trim()).filter(Boolean);
  return found.find((p) => p.toLowerCase().endsWith(".cmd")) ?? found[0];
}

const work = mkdtempSync(join(tmpdir(), "sps-probe-"));
ensureOpenssl();

// A certificate valid for both the local endpoint and the SPS hostname, so we
// can serve either name from one listener.
execFileSync("openssl", [
  "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "2",
  "-subj", "/CN=localhost", "-keyout", "k.pem", "-out", "c.pem",
  "-addext", "subjectAltName=DNS:localhost,DNS:app.vssps.visualstudio.com,IP:127.0.0.1",
], { cwd: work, stdio: ["ignore", "ignore", "pipe"] });

const entry = (id, area, resourceName, routeTemplate) => ({
  id, area, resourceName, routeTemplate,
  resourceVersion: 1, minVersion: "1.0", maxVersion: "7.2", releasedVersion: "7.1",
});

/**
 * Two discovery documents.
 *
 * `faithful` additionally advertises the `location` area — the entry `az` uses
 * to decide which host owns a service. `minimal` mirrors the earlier probe.
 */
const DOCUMENTS = {
  minimal: () => ({
    count: 1,
    value: [entry("603fe2ac-9723-48b9-88ad-09305aa6c6e1", "core", "projects", "_apis/{resource}/{*projectId}")],
  }),
  faithful: () => ({
    count: 3,
    value: [
      entry("603fe2ac-9723-48b9-88ad-09305aa6c6e1", "core", "projects", "_apis/{resource}/{*projectId}"),
      entry("e81700f7-3be2-46de-8624-2eb35882fcaa", "location", "resourceAreas", "_apis/{resource}/{areaId}"),
      entry("225f7195-f9c7-4d14-ab28-a83f7ff77e1f", "git", "repositories", "{project}/_apis/git/{resource}/{repositoryId}"),
    ],
  }),
};

/**
 * Resource-area lists returned from `/_apis/resourceAreas`.
 *
 * `sparse` is a single made-up entry (what the earlier probe served).
 * `complete` advertises the real Azure DevOps area GUIDs, every one pointing
 * back at our own endpoint — the best case for keeping `az` local.
 */
const AREA_LISTS = {
  sparse: (port) => [{ id: "x", name: "git", locationUrl: `https://localhost:${port}/${ORG}/` }],
  complete: (port) => {
    const local = `https://localhost:${port}/${ORG}/`;
    return [
      { id: "79134c72-4a58-4b42-976c-04e7115f32bf", name: "core", locationUrl: local },
      { id: "4e080c62-fa21-4fbc-8fef-2a10a2b38049", name: "git", locationUrl: local },
      { id: "5d6898bb-45ec-463f-95f9-54d49c71752e", name: "build", locationUrl: local },
      { id: "5264459e-e5e0-4bd8-b118-0985e68a4ec5", name: "wit", locationUrl: local },
      { id: "e81700f7-3be2-46de-8624-2eb35882fcaa", name: "location", locationUrl: local },
    ];
  },
};

const SCENARIOS = [
  ["minimal document + sparse areas", DOCUMENTS.minimal, AREA_LISTS.sparse],
  ["faithful document + sparse areas", DOCUMENTS.faithful, AREA_LISTS.sparse],
  ["faithful document + complete areas", DOCUMENTS.faithful, AREA_LISTS.complete],
];

async function runScenario(name, buildDocument, buildAreas) {
  const seen = [];
  const server = createServer(
    { key: readFileSync(join(work, "k.pem")), cert: readFileSync(join(work, "c.pem")) },
    (req, res) => {
      seen.push(`${req.method} ${req.headers.host}${req.url}`);
      const send = (o) => {
        const b = JSON.stringify(o);
        res.writeHead(200, { "content-type": "application/json", "content-length": Buffer.byteLength(b) });
        res.end(b);
      };
      if (req.method === "OPTIONS") return send(buildDocument());
      const path = req.url.split("?")[0].toLowerCase();
      // The location service: every area resolves back to *this* endpoint,
      // which is what should keep az from going to the real SPS.
      if (path.includes("/_apis/resourceareas")) {
        const port = server.address().port;
        const areas = buildAreas(port);
        return send({ count: areas.length, value: areas });
      }
      return send({ count: 1, value: [{ id: "1", name: "Widgets", state: "wellFormed" }] });
    });

  const port = await new Promise((r) => server.listen(0, "127.0.0.1", () => r(server.address().port)));

  // Point the SPS hostname at our own listener so a fallback is observable
  // rather than escaping to the real service.
  const originalLookup = dns.lookup;
  const spsHits = [];

  const az = resolveAz();
  const result = await new Promise((resolve) => {
    const child = spawn(`"${az}"`, [
      "devops", "project", "list",
      "--organization", `https://localhost:${port}/${ORG}`,
      "-o", "json", "--detect", "false", "--debug",
    ], {
      shell: true,
      env: {
        ...process.env,
        REQUESTS_CA_BUNDLE: join(work, "c.pem"),
        AZURE_DEVOPS_EXT_PAT: "dummy-pat-for-probe",
        AZURE_CORE_COLLECT_TELEMETRY: "no",
      },
    });
    let out = "", err = "";
    child.stdout.on("data", (c) => (out += c));
    child.stderr.on("data", (c) => (err += c));
    child.on("close", (code) => resolve({ code, out, err }));
  });

  dns.lookup = originalLookup;
  await new Promise((r) => server.close(r));

  // Which hosts did az actually try to reach?
  const hosts = new Set();
  const spsRequests = [];
  for (const line of result.err.split("\n")) {
    const m = line.match(/devops_sdk\.client:\s+(GET|OPTIONS|POST)\s+https:\/\/([^/\s]+)(\S*)/);
    if (m) {
      hosts.add(m[2]);
      if (m[2].includes("vssps") || m[2].includes("visualstudio.com")) {
        spsRequests.push(`${m[1]} ${m[3]}`);
      }
    }
  }

  return { name, port, seen, hosts: [...hosts], code: result.code, spsRequests, stderr: result.err };
}

console.log("Does a faithful resource-location document keep `az` off SPS?\n");

for (const [name, buildDocument, buildAreas] of SCENARIOS) {
  const r = await runScenario(name, buildDocument, buildAreas);
  const wentToSps = r.hosts.some((h) => h.includes("vssps") || h.includes("visualstudio.com"));
  console.log(`── ${name} ──`);
  console.log(`   az exit: ${r.code}`);
  console.log(`   hosts az addressed: ${r.hosts.join(", ") || "(none parsed)"}`);
  console.log(`   requests our endpoint served:`);
  for (const s of r.seen) console.log(`      ${s}`);
  console.log(`   REACHED SPS: ${wentToSps ? "YES" : "no"}`);
  if (r.spsRequests.length > 0) {
    console.log(`   what it asked SPS for:`);
    for (const s of r.spsRequests) console.log(`      ${s}`);
  }
  const notReg = r.stderr.split("\n").filter((l) => /not registered|resource location/i.test(l)).slice(0, 2);
  for (const l of notReg) console.log(`   note: ${l.trim().slice(0, 160)}`);
  console.log("");
}

rmSync(work, { recursive: true, force: true });
