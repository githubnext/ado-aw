/**
 * github-app-token — mint a GitHub App installation access token for the
 * Copilot engine (issue #1316).
 *
 * Mirrors gh-aw's `create-github-app-token` model, adapted to Azure DevOps:
 * the GitHub App **App ID** and **private key** are supplied via env vars
 * sourced from ADO pipeline (secret) variables, so no secret material ever
 * appears in the compiled pipeline source.
 *
 * Flow:
 *   1. Build a short-lived RS256 JWT signed with the App private key
 *      (`node:crypto` — no `openssl`, no npm dep).
 *   2. Resolve the installation ID for the configured owner
 *      (`GET /orgs/{owner}/installation`, falling back to
 *      `GET /users/{owner}/installation`).
 *   3. Exchange the JWT for an installation access token
 *      (`POST /app/installations/{id}/access_tokens`), optionally scoped to a
 *      set of repositories.
 *   4. Emit the token as a **masked, same-job** pipeline variable
 *      (`##vso[task.setvariable …;issecret=true]`) so a downstream step in the
 *      same job (the Copilot invocation) can read it via `$(GITHUB_APP_TOKEN)`
 *      without it leaking into the log.
 *
 * Trust boundary: runs OUTSIDE the AWF sandbox (a normal ADO script step, like
 * the exec-context bundles). It reaches the GitHub API host over the build
 * agent pool's normal network — no AWF allowlist entry is required. The private
 * key is read from the process env (set from a `$(VAR)` macro) and never
 * written to disk.
 *
 * Env-var contract (all set by the compiler's step `env:` block):
 *   - `GH_APP_ID`            (required) — the GitHub App ID.
 *   - `GH_APP_PRIVATE_KEY`   (required) — the App private key (PEM).
 *   - `GH_APP_OWNER`         (required) — installation owner (org or user).
 *   - `GH_APP_REPOSITORIES`  (optional) — comma/space/newline-separated repo
 *                            names to scope the token to.
 *   - `GH_APP_API_URL`       (optional) — API base URL (default
 *                            `https://api.github.com`; for GHES use
 *                            `https://<host>/api/v3`).
 *   - `GH_APP_OUTPUT_VAR`    (optional) — name of the masked variable to set
 *                            (default `GITHUB_APP_TOKEN`).
 */
import { createSign } from "node:crypto";

import { logError, logInfo, logWarning, setSecretVar } from "../shared/vso-logger.js";

const DEFAULT_API_URL = "https://api.github.com";
const DEFAULT_OUTPUT_VAR = "GITHUB_APP_TOKEN";
/** JWT lifetime: 9 minutes (GitHub caps App JWTs at 10). */
const JWT_TTL_SECONDS = 540;
/** Small clock-skew backdating for `iat` to tolerate agent/GitHub clock drift. */
const JWT_IAT_BACKDATE_SECONDS = 60;

function base64url(input: Buffer | string): string {
  const buf = typeof input === "string" ? Buffer.from(input, "utf8") : input;
  return buf
    .toString("base64")
    .replace(/=+$/g, "")
    .replace(/\+/g, "-")
    .replace(/\//g, "_");
}

/**
 * Build a signed RS256 JWT for authenticating as the GitHub App.
 * `nowSeconds` is injectable for deterministic tests.
 */
export function buildAppJwt(
  appId: string,
  privateKeyPem: string,
  nowSeconds: number = Math.floor(Date.now() / 1000),
): string {
  const header = { alg: "RS256", typ: "JWT" };
  const payload = {
    iat: nowSeconds - JWT_IAT_BACKDATE_SECONDS,
    exp: nowSeconds + JWT_TTL_SECONDS,
    iss: appId,
  };
  const signingInput = `${base64url(JSON.stringify(header))}.${base64url(
    JSON.stringify(payload),
  )}`;
  const signer = createSign("RSA-SHA256");
  signer.update(signingInput);
  signer.end();
  const signature = base64url(signer.sign(privateKeyPem));
  return `${signingInput}.${signature}`;
}

/** Parse the repositories env var into a clean list of names. */
export function parseRepositories(raw: string | undefined): string[] {
  if (!raw) return [];
  return raw
    .split(/[\s,]+/)
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

interface FetchLike {
  (
    url: string,
    init: {
      method: string;
      headers: Record<string, string>;
      body?: string;
    },
  ): Promise<{
    ok: boolean;
    status: number;
    json(): Promise<unknown>;
    text(): Promise<string>;
  }>;
}

function ghHeaders(bearer: string): Record<string, string> {
  return {
    Authorization: `Bearer ${bearer}`,
    Accept: "application/vnd.github+json",
    "X-GitHub-Api-Version": "2022-11-28",
    "User-Agent": "ado-aw-github-app-token",
  };
}

/**
 * Resolve the installation ID for `owner`. Tries the org endpoint first, then
 * the user endpoint (GitHub App installations exist on either an org or a
 * user account).
 */
export async function resolveInstallationId(
  fetchFn: FetchLike,
  apiUrl: string,
  jwt: string,
  owner: string,
): Promise<number> {
  const candidates = [
    `${apiUrl}/orgs/${encodeURIComponent(owner)}/installation`,
    `${apiUrl}/users/${encodeURIComponent(owner)}/installation`,
  ];
  let lastStatus = 0;
  let lastBody = "";
  for (const url of candidates) {
    const resp = await fetchFn(url, {
      method: "GET",
      headers: ghHeaders(jwt),
    });
    if (resp.ok) {
      const data = (await resp.json()) as { id?: number };
      if (typeof data.id === "number") {
        return data.id;
      }
      throw new Error(
        `installation lookup for '${owner}' returned no numeric id`,
      );
    }
    lastStatus = resp.status;
    lastBody = await resp.text();
  }
  throw new Error(
    `could not resolve a GitHub App installation for owner '${owner}' ` +
      `(last HTTP ${lastStatus}): ${lastBody}`,
  );
}

/**
 * Exchange the App JWT for an installation access token, optionally scoped to
 * `repositories`.
 */
export async function mintInstallationToken(
  fetchFn: FetchLike,
  apiUrl: string,
  jwt: string,
  installationId: number,
  repositories: string[],
): Promise<string> {
  const body: Record<string, unknown> = {};
  if (repositories.length > 0) {
    body.repositories = repositories;
  }
  const resp = await fetchFn(
    `${apiUrl}/app/installations/${installationId}/access_tokens`,
    {
      method: "POST",
      headers: ghHeaders(jwt),
      body: JSON.stringify(body),
    },
  );
  if (!resp.ok) {
    const text = await resp.text();
    throw new Error(
      `failed to mint installation token (HTTP ${resp.status}): ${text}`,
    );
  }
  const data = (await resp.json()) as { token?: string };
  if (!data.token || data.token.length === 0) {
    throw new Error("installation token response contained no token");
  }
  return data.token;
}

function requireEnv(env: NodeJS.ProcessEnv, key: string): string {
  const value = env[key];
  if (!value || value.length === 0) {
    throw new Error(`required env var ${key} is missing or empty`);
  }
  return value;
}

function resolveApiUrl(env: NodeJS.ProcessEnv): string {
  return (
    env.GH_APP_API_URL && env.GH_APP_API_URL.length > 0
      ? env.GH_APP_API_URL
      : DEFAULT_API_URL
  ).replace(/\/+$/, "");
}

/**
 * Revoke (delete) the installation access token via
 * `DELETE /installation/token`, authenticated with the token itself.
 *
 * Fully best-effort: a missing token or a failed request is logged as a
 * warning and still returns 0, so token revocation can never fail the build
 * (the caller also marks the step `continueOnError`). Reads the minted token
 * from `GH_APP_TOKEN` (the masked same-job variable set by the mint step).
 */
export async function revoke(
  env: NodeJS.ProcessEnv = process.env,
  fetchFn: FetchLike = fetch as unknown as FetchLike,
): Promise<number> {
  const token = env.GH_APP_TOKEN;
  if (!token || token.length === 0) {
    logWarning("[github-app-token] no token to revoke (GH_APP_TOKEN empty)");
    return 0;
  }
  const apiUrl = resolveApiUrl(env);
  try {
    const resp = await fetchFn(`${apiUrl}/installation/token`, {
      method: "DELETE",
      headers: ghHeaders(token),
    });
    if (resp.ok) {
      logInfo("[github-app-token] revoked installation token");
    } else {
      logWarning(
        `[github-app-token] revoke returned HTTP ${resp.status} (ignored)`,
      );
    }
  } catch (err) {
    logWarning(
      `[github-app-token] revoke failed (ignored): ${
        err instanceof Error ? err.message : String(err)
      }`,
    );
  }
  return 0;
}

export async function main(
  env: NodeJS.ProcessEnv = process.env,
  fetchFn: FetchLike = fetch as unknown as FetchLike,
): Promise<number> {
  try {
    const appId = requireEnv(env, "GH_APP_ID");
    const privateKey = requireEnv(env, "GH_APP_PRIVATE_KEY");
    const owner = requireEnv(env, "GH_APP_OWNER");
    const repositories = parseRepositories(env.GH_APP_REPOSITORIES);
    const apiUrl = resolveApiUrl(env);
    const outputVar =
      env.GH_APP_OUTPUT_VAR && env.GH_APP_OUTPUT_VAR.length > 0
        ? env.GH_APP_OUTPUT_VAR
        : DEFAULT_OUTPUT_VAR;

    const jwt = buildAppJwt(appId, privateKey);
    const installationId = await resolveInstallationId(
      fetchFn,
      apiUrl,
      jwt,
      owner,
    );
    const token = await mintInstallationToken(
      fetchFn,
      apiUrl,
      jwt,
      installationId,
      repositories,
    );

    // Mask + expose to the same-job Copilot step. Emitting the secret BEFORE
    // any log line that could contain it keeps ADO's scrubber ahead of leaks.
    setSecretVar(outputVar, token);
    logInfo(
      `[github-app-token] minted installation token for owner '${owner}' ` +
        `(installation ${installationId}, ${
          repositories.length > 0
            ? `${repositories.length} repo(s)`
            : "all repos"
        }) -> $(${outputVar})`,
    );
    return 0;
  } catch (err) {
    logError(
      `[github-app-token] ${err instanceof Error ? err.message : String(err)}`,
    );
    return 1;
  }
}

// CLI entry guard: only run when invoked directly (not when imported by tests).
// A `revoke` argument switches to token-revocation mode; otherwise the default
// mint flow runs. Uses argv comparison rather than a top-level await so the
// bundle stays CJS.
if (
  typeof process !== "undefined" &&
  process.argv[1] &&
  /github-app-token(\/index)?\.js$/.test(process.argv[1])
) {
  const run = process.argv[2] === "revoke" ? revoke() : main();
  run.then((rc) => process.exit(rc));
}
