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
 * Invocation contract. Compiler-owned, **non-secret** inputs are passed as argv
 * flags (immune to ADO pipeline-variable shadowing — see `CliArgs`); the two
 * **secrets** stay in env as `$(secret)` macros so ADO masks them and they never
 * appear in the step's command line.
 *
 *   Mint:   node github-app-token.js \
 *             --app-id <id> --owner <login> --output-var <name> \
 *             [--repositories "a b"] [--api-url https://host/api/v3]
 *           env: GH_APP_PRIVATE_KEY (required, secret)
 *
 *   Revoke: node github-app-token.js revoke [--api-url https://host/api/v3]
 *           env: GH_APP_TOKEN (the minted token, secret)
 *
 * Flags: `--app-id` the GitHub App ID; `--owner` installation owner (org/user);
 * `--output-var` the masked variable name to set (compiler-pinned, defaults to
 * `GITHUB_APP_TOKEN`); `--repositories` space/comma-separated repo names to
 * scope the token to; `--api-url` API base URL (default `https://api.github.com`,
 * GHES uses `https://<host>/api/v3`).
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
  const normalizedPrivateKeyPem = normalizePrivateKeyPem(privateKeyPem);
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
  const signature = base64url(signer.sign(normalizedPrivateKeyPem));
  return `${signingInput}.${signature}`;
}

/**
 * Normalize private-key PEM inputs commonly seen from ADO secret variables:
 * - escaped newlines (`\\n`, `\\r\\n`, `\\r`)
 * - CRLF/CR endings
 * - whitespace-collapsed PEM bodies
 */
export function normalizePrivateKeyPem(rawPem: string): string {
  const normalizedNewlines = rawPem
    .replace(/\\r\\n/g, "\n")
    .replace(/\\n/g, "\n")
    .replace(/\\r/g, "\n")
    .replace(/\r/g, "\n")
    .trim();
  const match = normalizedNewlines.match(
    /-----BEGIN ([^-]+)-----([\s\S]*?)-----END \1-----/,
  );
  if (!match) {
    return normalizedNewlines;
  }
  const label = match[1];
  const body = match[2] ?? "";
  const compactBody = body.replace(/\s+/g, "");
  const wrappedBody = compactBody.match(/.{1,64}/g)?.join("\n") ?? "";
  return `-----BEGIN ${label}-----\n${wrappedBody}\n-----END ${label}-----`;
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

/** Normalize an optional API base URL, defaulting to GHEC and stripping any
 *  trailing slashes. */
function normalizeApiUrl(raw: string | undefined): string {
  return (raw && raw.length > 0 ? raw : DEFAULT_API_URL).replace(/\/+$/, "");
}

/**
 * Options that steer the mint/revoke flows. These are **compiler-owned,
 * non-secret** inputs passed as argv flags — deliberately NOT read from
 * `process.env`. ADO injects every pipeline variable into a step's env, so an
 * env-sourced knob (e.g. the output-variable name) could be silently shadowed
 * by a same-named pipeline variable and, in the worst case, redirect the minted
 * token. Argv comes only from the compiler-authored step script, so it cannot
 * be shadowed. Secret material (`GH_APP_PRIVATE_KEY`, and the minted
 * `GH_APP_TOKEN` for revoke) stays in env as `$(secret)` macros so ADO masks it
 * and it never appears in the step's command line.
 */
export interface CliArgs {
  appId?: string;
  owner?: string;
  outputVar?: string;
  repositories?: string;
  apiUrl?: string;
}

/**
 * Parse the `--flag value` argv (after any leading `revoke` subcommand) into a
 * flat `CliArgs`. Unknown flags are ignored so the compiler can add new ones
 * without breaking an older bundle. Repeated flags are last-write-wins.
 *
 * Forward-compatibility: a token that looks like a flag (`--…`) is never
 * consumed as the *value* of a preceding flag. This keeps parsing aligned even
 * if a future compiler emits a value-less boolean flag (e.g. `--debug`) — the
 * boolean is skipped by itself rather than swallowing the next real flag.
 */
export function parseArgs(argv: string[]): CliArgs {
  const out: CliArgs = {};
  let i = 0;
  while (i < argv.length) {
    const flag = argv[i];
    const next = argv[i + 1];
    // A `--…` token is a flag, not a value: treat the current flag as valueless
    // and advance by one so the next flag stays aligned.
    const value = next !== undefined && !next.startsWith("--") ? next : undefined;
    switch (flag) {
      case "--app-id":
        if (value !== undefined) out.appId = value;
        break;
      case "--owner":
        if (value !== undefined) out.owner = value;
        break;
      case "--output-var":
        if (value !== undefined) out.outputVar = value;
        break;
      case "--repositories":
        if (value !== undefined) out.repositories = value;
        break;
      case "--api-url":
        if (value !== undefined) out.apiUrl = value;
        break;
      default:
        break;
    }
    i += value !== undefined ? 2 : 1;
  }
  return out;
}

function requireArg(value: string | undefined, flag: string): string {
  if (!value || value.length === 0) {
    throw new Error(`required argument ${flag} is missing or empty`);
  }
  return value;
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
  args: CliArgs,
  env: NodeJS.ProcessEnv = process.env,
  fetchFn: FetchLike = fetch as unknown as FetchLike,
): Promise<number> {
  const token = env.GH_APP_TOKEN;
  if (!token || token.length === 0) {
    logWarning("[github-app-token] no token to revoke (GH_APP_TOKEN empty)");
    return 0;
  }
  const apiUrl = normalizeApiUrl(args.apiUrl);
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
  args: CliArgs,
  env: NodeJS.ProcessEnv = process.env,
  fetchFn: FetchLike = fetch as unknown as FetchLike,
): Promise<number> {
  try {
    // Non-secret, compiler-owned inputs come from argv (immune to pipeline-var
    // shadowing); only the private key is read from the masked env.
    const appId = requireArg(args.appId, "--app-id");
    const owner = requireArg(args.owner, "--owner");
    const privateKey = requireEnv(env, "GH_APP_PRIVATE_KEY");
    const repositories = parseRepositories(args.repositories);
    const apiUrl = normalizeApiUrl(args.apiUrl);
    const outputVar =
      args.outputVar && args.outputVar.length > 0
        ? args.outputVar
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
// A leading `revoke` argument switches to token-revocation mode; otherwise the
// default mint flow runs. Compiler-owned inputs are argv flags; secrets stay in
// env. Uses argv comparison rather than a top-level await so the bundle stays
// CJS.
if (
  typeof process !== "undefined" &&
  process.argv[1] &&
  /github-app-token(\/index)?\.js$/.test(process.argv[1])
) {
  const rest = process.argv.slice(2);
  const isRevoke = rest[0] === "revoke";
  const args = parseArgs(isRevoke ? rest.slice(1) : rest);
  const run = isRevoke ? revoke(args) : main(args);
  run.then((rc) => process.exit(rc));
}
