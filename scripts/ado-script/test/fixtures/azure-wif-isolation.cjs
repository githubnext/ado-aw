const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { spawnSync } = require("node:child_process");

async function main() {
  const { runRefresher } = await import("./refresher.mjs");
  const state = "/state";
  const owner = 10001;
  fs.chmodSync("/inputs", 0o755);
  fs.chownSync(state, owner, owner);
  fs.writeFileSync(path.join(state, "public"), "host-view-control");
  process.setgid(owner);
  process.setuid(owner);

  const setup = spawnSync("bash", ["/inputs/setup.sh"], {
    encoding: "utf8",
    env: {
      PATH: process.env.PATH,
      idToken: "synthetic-initial-assertion",
      servicePrincipalId: "11111111-2222-3333-4444-555555555555",
      tenantId: "11111111-2222-3333-4444-555555555555",
      AZURESUBSCRIPTION_SERVICE_CONNECTION_ID: "11111111-2222-3333-4444-555555555555",
      SYSTEM_ACCESSTOKEN: "synthetic-job-token",
      SYSTEM_OIDCREQUESTURI: "https://example.invalid/oidc",
    },
  });
  assert.equal(setup.status, 0, setup.stderr);
  const authRoot = path.join(state, "ado-aw-azure-auth");
  const serverDirs = fs.readdirSync(authRoot);
  assert.equal(serverDirs.length, 1);
  const auth = path.join(authRoot, serverDirs[0]);
  fs.writeFileSync(path.join(state, "auth-path"), auth);
  assert.equal(fs.statSync(authRoot).mode & 0o777, 0o700);
  assert.equal(fs.statSync(auth).mode & 0o777, 0o700);
  assert.equal(fs.statSync(path.join(auth, "token.d")).mode & 0o777, 0o755);
  assert.equal(fs.statSync(path.join(auth, "material")).mode & 0o777, 0o600);
  assert.ok(fs.statSync(path.join(auth, "material")).isFIFO());

  let now = 1_700_000_000_000;
  const jwt = (exp) => `eyJhbGciOiJub25lIn0.${Buffer.from(JSON.stringify({ exp })).toString("base64url")}.fake`;
  const controller = new AbortController();
  let sleepCount = 0;
  const code = await runRefresher({
    initialIdToken: jwt(now / 1000 + 300),
    systemAccessToken: "synthetic-job-token",
    oidcRequestUri: "https://example.invalid/oidc",
    serviceConnectionId: "11111111-2222-3333-4444-555555555555",
    tokenPath: path.join(auth, "token.d/token"),
    readyPath: path.join(auth, "ready.json"),
    statusPath: path.join(auth, "status.json"),
  }, controller.signal, {
    now: () => now,
    provider: { createOidcToken: async () => jwt(now / 1000 + 300) },
    sleep: async (ms) => {
      const marker = path.join(state, `advance-${++sleepCount}`);
      const deadline = Date.now() + 60_000;
      while (!fs.existsSync(marker)) {
        if (fs.existsSync(path.join(state, "stop"))) {
          controller.abort();
          return;
        }
        assert.ok(Date.now() < deadline, "test controller did not advance the refresher");
        await new Promise((resolve) => setTimeout(resolve, 25));
      }
      now += ms;
    },
  });
  assert.equal(code, 0);
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
