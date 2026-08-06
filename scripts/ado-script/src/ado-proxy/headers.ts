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
  const headers: Record<string, string> = {};
  for (const [rawName, rawValue] of Object.entries(incoming)) {
    const name = rawName.toLowerCase();
    if (!FORWARDED_RESPONSE_HEADERS.has(name)) continue;
    const value = firstValue(rawValue);
    if (value !== undefined) headers[name] = value;
  }
  return headers;
}

export const INTERNAL = {
  FORWARDED_REQUEST_HEADERS,
  FORWARDED_RESPONSE_HEADERS,
  CREDENTIAL_HEADERS,
};
