/**
 * Header handling for protected (TLS-terminated) requests.
 *
 * Two jobs, both fail-closed:
 *
 *   1. **Strip every client-supplied credential.** The agent may set
 *      `Authorization`, a sentinel PAT, cookies, or an auth-like proxy header;
 *      none of it may influence the upstream call. The proxy's injected bearer
 *      is the only credential that ever reaches Azure DevOps.
 *   2. **Forward only known-safe headers.** An allowlist rather than a denylist,
 *      so a header nobody thought about (`X-HTTP-Method-Override`,
 *      `X-Original-URL`, a smuggled `Transfer-Encoding`) cannot change what the
 *      upstream believes the request is.
 */

/**
 * Request headers forwarded upstream, lowercased.
 *
 * Deliberately small. Anything Azure DevOps genuinely needs for content
 * negotiation, correlation, or paging is here; everything else is dropped
 * because the request the policy authorized must be the request that is sent.
 */
const FORWARDED_REQUEST_HEADERS: ReadonlySet<string> = new Set([
  // Content negotiation. `accept` also carries the api-version parameter that
  // `resolveApiVersion` validates, so it must survive intact.
  "accept",
  "accept-language",
  "content-type",
  // Client identification, useful in upstream diagnostics and harmless.
  "user-agent",
  // Azure DevOps correlation/session headers. Dropping these degrades server
  // -side tracing and makes some SDK paths chattier, but they carry no
  // authority.
  "x-tfs-session",
  "x-vss-e2eid",
  "x-vss-usersessionid",
  // Paging. Without this a continued list restarts from the beginning.
  "x-ms-continuationtoken",
]);

/**
 * Response headers returned to the client, lowercased.
 *
 * Also an allowlist: upstream `set-cookie`, `www-authenticate`, and redirect
 * `location` headers must never reach the agent. The first two would hand it
 * session material or provoke an interactive login; the third is how a signed
 * artifact URL escapes.
 */
const FORWARDED_RESPONSE_HEADERS: ReadonlySet<string> = new Set([
  "content-type",
  "x-ms-continuationtoken",
  "x-vss-e2eid",
  "retry-after",
]);

/**
 * Headers whose presence is logged as a stripped credential.
 *
 * Only used for observability — everything outside the allowlist is dropped
 * regardless. Naming these lets the audit stream distinguish "the agent tried
 * to supply its own credential" from ordinary header noise.
 */
const CREDENTIAL_HEADERS: readonly string[] = [
  "authorization",
  "proxy-authorization",
  "cookie",
  "cookie2",
  "x-tfs-fedauthredirect",
  "www-authenticate",
];

/** Result of sanitizing a client request's headers. */
export interface SanitizedHeaders {
  /** Headers to send upstream, already including the protocol headers. */
  readonly headers: Readonly<Record<string, string>>;
  /** Names of credential-bearing headers the client supplied, for the log. */
  readonly strippedCredentials: readonly string[];
}

function firstValue(value: string | string[] | undefined): string | undefined {
  if (value === undefined) return undefined;
  // Node folds most repeated headers into one comma-joined string, but not
  // `set-cookie`. Take the first: a header repeated with different values is
  // exactly the ambiguity an upstream might resolve differently than we do.
  return Array.isArray(value) ? value[0] : value;
}

/**
 * Build the upstream header set for an authorized request.
 *
 * The bearer is applied by the caller *after* the allow decision; this function
 * never sees it, so no code path can accidentally emit it on a denial.
 */
export function sanitizeRequestHeaders(
  incoming: Readonly<Record<string, string | string[] | undefined>>,
  host: string,
): SanitizedHeaders {
  const headers: Record<string, string> = {};
  const strippedCredentials: string[] = [];

  for (const [rawName, rawValue] of Object.entries(incoming)) {
    const name = rawName.toLowerCase();
    if (CREDENTIAL_HEADERS.includes(name)) {
      strippedCredentials.push(name);
      continue;
    }
    if (!FORWARDED_REQUEST_HEADERS.has(name)) continue;
    const value = firstValue(rawValue);
    if (value !== undefined) headers[name] = value;
  }

  headers.host = host;
  // Without this Azure DevOps answers an unauthenticated or under-privileged
  // request with a 203 and a sign-in page instead of a 401, which clients
  // surface as unparseable HTML rather than an auth failure.
  headers["x-tfs-fedauthredirect"] = "Suppress";
  // Identity encoding keeps response filtering and the byte budget honest; the
  // hop to the agent is loopback-adjacent, so the saving is not worth the
  // decompression bomb surface.
  headers["accept-encoding"] = "identity";
  headers.connection = "close";

  return { headers, strippedCredentials };
}

/** Filter an upstream response's headers down to the safe set. */
export function sanitizeResponseHeaders(
  incoming: Readonly<Record<string, string | string[] | undefined>>,
): Record<string, string> {
  return pickHeaders(incoming, FORWARDED_RESPONSE_HEADERS);
}

function pickHeaders(
  incoming: Readonly<Record<string, string | string[] | undefined>>,
  allowed: ReadonlySet<string>,
): Record<string, string> {
  const headers: Record<string, string> = {};
  for (const [rawName, rawValue] of Object.entries(incoming)) {
    const name = rawName.toLowerCase();
    if (!allowed.has(name)) continue;
    const value = firstValue(rawValue);
    if (value !== undefined) headers[name] = value;
  }
  return headers;
}

/**
 * Request headers forwarded to a package host, lowercased.
 *
 * Package clients need conditional and ranged fetches (restore caches, resumed
 * downloads) and send a few protocol-identification headers. Everything else
 * is dropped for the same reason as on the REST path: the request the policy
 * authorized must be the request that is sent. Notably absent:
 *
 *   - `authorization`, `cookie`, `proxy-*` — client credentials, never relayed;
 *   - `x-nuget-apikey` — NuGet's push credential, and a push is never allowed;
 *   - `npm-otp`, `npm-auth-type` — interactive-auth material;
 *   - `referer` — npm puts the invoking command line there, which upstream
 *     does not need;
 *   - `forwarded` / `x-forwarded-*` and hop-by-hop headers.
 */
const PACKAGE_REQUEST_HEADERS: ReadonlySet<string> = new Set([
  "accept",
  "accept-language",
  "user-agent",
  "cache-control",
  "pragma",
  "if-none-match",
  "if-modified-since",
  "if-match",
  "if-unmodified-since",
  "range",
  "if-range",
  // npm protocol identification.
  "npm-command",
  "npm-scope",
  "npm-session",
  "npm-in-ci",
  // NuGet protocol identification. An explicit list rather than an
  // `x-nuget-*` prefix so `x-nuget-apikey` can never slip through.
  "x-nuget-session-id",
  "x-nuget-client-version",
  "x-nuget-protocol-version",
]);

/**
 * Response headers returned from a package host, lowercased.
 *
 * Enough for clients to cache, resume, and name downloads. `set-cookie` and
 * `www-authenticate` are never relayed (session material, interactive-login
 * prompts), and `location` is handled separately by the redirect validator.
 */
const PACKAGE_RESPONSE_HEADERS: ReadonlySet<string> = new Set([
  "content-type",
  "content-length",
  // Relayed in case the upstream ignores `accept-encoding: identity`;
  // dropping it would hand the client compressed bytes it believes are plain.
  "content-encoding",
  "etag",
  "last-modified",
  "cache-control",
  "expires",
  "date",
  "accept-ranges",
  "content-range",
  "content-disposition",
  "vary",
  "x-content-type-options",
  "retry-after",
]);

/** Credential-like headers a package client may send, logged when stripped. */
const PACKAGE_CREDENTIAL_HEADERS: readonly string[] = [
  ...CREDENTIAL_HEADERS,
  "x-nuget-apikey",
  "npm-otp",
];

/**
 * Build the upstream header set for an authorized package request.
 *
 * As with {@link sanitizeRequestHeaders}, the credential is applied by the
 * caller after the allow decision; this function never sees it.
 */
export function sanitizePackageRequestHeaders(
  incoming: Readonly<Record<string, string | string[] | undefined>>,
  host: string,
): SanitizedHeaders {
  const strippedCredentials: string[] = [];
  for (const rawName of Object.keys(incoming)) {
    const name = rawName.toLowerCase();
    if (PACKAGE_CREDENTIAL_HEADERS.includes(name)) strippedCredentials.push(name);
  }

  const headers = pickHeaders(incoming, PACKAGE_REQUEST_HEADERS);
  headers.host = host;
  // Azure Artifacts shares Azure DevOps' identity front end: without this an
  // under-privileged request may get a 203 sign-in page instead of a 401.
  headers["x-tfs-fedauthredirect"] = "Suppress";
  // Keeps the response byte budget honest: the limit is on what the client
  // receives, so it must not be defeated by a compressed transfer.
  headers["accept-encoding"] = "identity";
  headers.connection = "close";

  return { headers, strippedCredentials };
}

/** Filter a package host's response headers down to the safe set. */
export function sanitizePackageResponseHeaders(
  incoming: Readonly<Record<string, string | string[] | undefined>>,
): Record<string, string> {
  return pickHeaders(incoming, PACKAGE_RESPONSE_HEADERS);
}

export const INTERNAL = {
  FORWARDED_REQUEST_HEADERS,
  FORWARDED_RESPONSE_HEADERS,
  CREDENTIAL_HEADERS,
  PACKAGE_REQUEST_HEADERS,
  PACKAGE_RESPONSE_HEADERS,
};
