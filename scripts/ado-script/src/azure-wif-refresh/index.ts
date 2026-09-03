/**
 * azure-wif-refresh — maintain a rotating Azure federated assertion file.
 *
 * The trusted host writes one JSON material document to stdin. This sidecar
 * keeps the Azure DevOps bearer in memory, publishes only the federated
 * assertion, and refreshes it before expiry for an MCP container that mounts
 * the token path read-only.
 */
import { randomUUID } from "node:crypto";
import {
  chmod,
  mkdir,
  open,
  rename,
  unlink,
} from "node:fs/promises";
import { basename, dirname, join } from "node:path";
import type { Readable } from "node:stream";

const REFRESH_SKEW_MS = 60_000;
const FALLBACK_REFRESH_MS = 4 * 60_000;
const FALLBACK_VALIDITY_MS = 5 * 60_000;
const INITIAL_RETRY_MS = 1_000;
const MAX_RETRY_MS = 30_000;
const REQUEST_TIMEOUT_MS = 30_000;
const MAX_MATERIAL_BYTES = 1024 * 1024;

const MATERIAL_FIELDS = [
  "initialIdToken",
  "systemAccessToken",
  "oidcRequestUri",
  "serviceConnectionId",
  "tokenPath",
  "readyPath",
  "statusPath",
] as const;

type MaterialField = (typeof MATERIAL_FIELDS)[number];

export interface RefreshMaterial {
  readonly initialIdToken: string;
  readonly systemAccessToken: string;
  readonly oidcRequestUri: string;
  readonly serviceConnectionId: string;
  readonly tokenPath: string;
  readonly readyPath: string;
  readonly statusPath: string;
}

export type ErrorCategory =
  | "timeout"
  | "network"
  | "throttled"
  | "server"
  | "client"
  | "invalid-response"
  | "filesystem"
  | "unknown";

export type SidecarState =
  | "starting"
  | "ready"
  | "refreshing"
  | "unhealthy"
  | "stopped";

export interface StatusDocument {
  readonly state: SidecarState;
  readonly updatedAt: string;
  readonly assertionExpiresAt?: string;
  readonly nextRefreshAt?: string;
  readonly lastRefreshAt?: string;
  readonly stoppedAt?: string;
  readonly errorCategory?: ErrorCategory;
}

export interface ReadyDocument {
  readonly state: "ready";
  readonly readyAt: string;
  readonly assertionExpiresAt: string;
}

interface AssertionTiming {
  readonly expiresAt: number;
  readonly refreshAt: number;
  readonly fallback: boolean;
}

export interface OidcProvider {
  createOidcToken(material: RefreshMaterial): Promise<unknown>;
}

export type AtomicWriter = (
  path: string,
  content: string,
  mode: number,
) => Promise<void>;

export interface RuntimeDependencies {
  readonly now?: () => number;
  readonly sleep?: (ms: number, signal: AbortSignal) => Promise<void>;
  readonly provider?: OidcProvider;
  readonly writeAtomic?: AtomicWriter;
  readonly report?: (message: string) => void;
  readonly requestTimeoutMs?: number;
}

export class MaterialError extends Error {}
export class ShutdownError extends Error {}
class RequestTimeoutError extends Error {}
class InvalidResponseError extends Error {}

function asRecord(value: unknown): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new MaterialError("material must be a JSON object");
  }
  return value as Record<string, unknown>;
}

function requireNonemptyString(
  source: Record<string, unknown>,
  field: MaterialField,
): string {
  const value = source[field];
  if (typeof value !== "string" || value.trim() === "") {
    throw new MaterialError(`${field} must be a non-empty string`);
  }
  return value;
}

function requireGuid(
  source: Record<string, unknown>,
  field: "serviceConnectionId",
): string {
  const value = requireNonemptyString(source, field);
  if (
    !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(
      value,
    )
  ) {
    throw new MaterialError(`${field} must be a GUID`);
  }
  return value;
}

/** Parse and strictly validate the one-shot stdin material document. */
export function parseMaterial(raw: string): RefreshMaterial {
  if (raw.trim() === "") {
    throw new MaterialError("no material on stdin");
  }

  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    throw new MaterialError("material is not valid JSON");
  }

  const source = asRecord(parsed);
  const allowed = new Set<string>(MATERIAL_FIELDS);
  if (Object.keys(source).some((key) => !allowed.has(key))) {
    throw new MaterialError("material contains unknown fields");
  }

  return {
    initialIdToken: requireNonemptyString(source, "initialIdToken"),
    systemAccessToken: requireNonemptyString(source, "systemAccessToken"),
    oidcRequestUri: requireNonemptyString(source, "oidcRequestUri"),
    serviceConnectionId: requireGuid(source, "serviceConnectionId"),
    tokenPath: requireNonemptyString(source, "tokenPath"),
    readyPath: requireNonemptyString(source, "readyPath"),
    statusPath: requireNonemptyString(source, "statusPath"),
  };
}

/**
 * Read exactly one top-level JSON object, then detach from stdin without
 * waiting for the producer to close the pipe.
 */
export function readOneJsonDocument(
  input: Readable = process.stdin,
): Promise<string> {
  return new Promise<string>((resolve, reject) => {
    let buffer = "";
    let started = false;
    let depth = 0;
    let inString = false;
    let escaped = false;

    const cleanup = (): void => {
      input.off("data", onData);
      input.off("end", onEnd);
      input.off("error", onError);
      input.pause();
    };

    const fail = (message: string): void => {
      cleanup();
      reject(new MaterialError(message));
    };

    const onData = (chunk: Buffer | string): void => {
      const text = chunk.toString();
      if (Buffer.byteLength(buffer) + Buffer.byteLength(text) > MAX_MATERIAL_BYTES) {
        fail("material exceeds the size limit");
        return;
      }
      const previousLength = buffer.length;
      buffer += text;

      for (let i = previousLength; i < buffer.length; i += 1) {
        const char = buffer[i]!;
        if (!started) {
          if (/\s/.test(char)) continue;
          if (char !== "{") {
            fail("material must be a JSON object");
            return;
          }
          started = true;
          depth = 1;
          continue;
        }
        if (inString) {
          if (escaped) {
            escaped = false;
          } else if (char === "\\") {
            escaped = true;
          } else if (char === '"') {
            inString = false;
          }
          continue;
        }
        if (char === '"') {
          inString = true;
        } else if (char === "{" || char === "[") {
          depth += 1;
        } else if (char === "}" || char === "]") {
          depth -= 1;
          if (depth === 0) {
            const document = buffer.slice(0, i + 1);
            if (buffer.slice(i + 1).trim() !== "") {
              fail("material contains trailing data");
              return;
            }
            cleanup();
            resolve(document);
            return;
          }
          if (depth < 0) {
            fail("material is not valid JSON");
            return;
          }
        }
      }
    };

    const onEnd = (): void => {
      fail("stdin ended before a complete material document was received");
    };
    const onError = (): void => {
      fail("cannot read material from stdin");
    };

    input.setEncoding("utf8");
    input.on("data", onData);
    input.once("end", onEnd);
    input.once("error", onError);
    input.resume();
  });
}

/** Decode a JWT expiry without verifying its signature. */
export function parseJwtExpiryMs(token: string): number | undefined {
  const segments = token.split(".");
  if (segments.length !== 3 || !segments[1]) return undefined;
  try {
    const payload: unknown = JSON.parse(
      Buffer.from(segments[1], "base64url").toString("utf8"),
    );
    if (typeof payload !== "object" || payload === null || Array.isArray(payload)) {
      return undefined;
    }
    const exp = (payload as Record<string, unknown>).exp;
    if (
      typeof exp !== "number" ||
      !Number.isSafeInteger(exp) ||
      exp <= 0 ||
      exp > Math.floor(Number.MAX_SAFE_INTEGER / 1000)
    ) {
      return undefined;
    }
    return exp * 1000;
  } catch {
    return undefined;
  }
}

export function assertionTiming(token: string, now: number): AssertionTiming {
  const expiresAt = parseJwtExpiryMs(token);
  if (expiresAt === undefined) {
    return {
      expiresAt: now + FALLBACK_VALIDITY_MS,
      refreshAt: now + FALLBACK_REFRESH_MS,
      fallback: true,
    };
  }
  return {
    expiresAt,
    refreshAt: Math.max(now, expiresAt - REFRESH_SKEW_MS),
    fallback: false,
  };
}

/** Replace a file atomically using a private same-directory temporary file. */
export async function writeAtomic(
  path: string,
  content: string,
  mode: number,
): Promise<void> {
  const directory = dirname(path);
  await mkdir(directory, { recursive: true });
  const temporaryPath = join(
    directory,
    `.${basename(path)}.${process.pid}.${randomUUID()}.tmp`,
  );
  let handle;
  try {
    handle = await open(temporaryPath, "wx", mode);
    await handle.writeFile(content, "utf8");
    await handle.sync();
    await handle.chmod(mode);
    await handle.close();
    handle = undefined;
    await rename(temporaryPath, path);
    await chmod(path, mode);
  } catch (error) {
    await handle?.close().catch(() => undefined);
    await unlink(temporaryPath).catch(() => undefined);
    throw error;
  }
}

function defaultReport(message: string): void {
  process.stderr.write(`[azure-wif-refresh] ${message}\n`);
}

function defaultSleep(ms: number, signal: AbortSignal): Promise<void> {
  if (signal.aborted) return Promise.reject(new ShutdownError());
  return new Promise<void>((resolve, reject) => {
    const timer = setTimeout(done, ms);

    function done(): void {
      signal.removeEventListener("abort", aborted);
      resolve();
    }
    function aborted(): void {
      clearTimeout(timer);
      signal.removeEventListener("abort", aborted);
      reject(new ShutdownError());
    }

    signal.addEventListener("abort", aborted, { once: true });
  });
}

export interface FetchLike {
  (
    url: string,
    init: {
      method: "POST";
      headers: Record<string, string>;
      body: string;
    },
  ): Promise<{
    ok: boolean;
    status: number;
    json(): Promise<unknown>;
  }>;
}

class HttpResponseError extends Error {
  readonly statusCode: number;

  constructor(statusCode: number) {
    super("OIDC endpoint returned a non-success status");
    this.statusCode = statusCode;
  }
}

/** Request a fresh assertion from the job-scoped Azure DevOps OIDC endpoint. */
export async function requestOidcToken(
  material: RefreshMaterial,
  fetchFn: FetchLike = fetch as unknown as FetchLike,
): Promise<string> {
  const url =
    `${material.oidcRequestUri}?api-version=7.1&serviceConnectionId=` +
    encodeURIComponent(material.serviceConnectionId);
  const response = await fetchFn(url, {
    method: "POST",
    headers: {
      Authorization: `Bearer ${material.systemAccessToken}`,
      "Content-Type": "application/json",
      "X-TFS-FedAuthRedirect": "Suppress",
    },
    body: "{}",
  });
  if (!response.ok) {
    throw new HttpResponseError(response.status);
  }
  const body: unknown = await response.json();
  if (typeof body !== "object" || body === null || Array.isArray(body)) {
    throw new InvalidResponseError();
  }
  return extractOidcToken((body as Record<string, unknown>).oidcToken);
}

class AzureDevOpsOidcProvider implements OidcProvider {
  async createOidcToken(material: RefreshMaterial): Promise<unknown> {
    return await requestOidcToken(material);
  }
}

function httpStatusCode(error: unknown): number | undefined {
  if (!error || typeof error !== "object") return undefined;
  const value = error as {
    statusCode?: unknown;
    response?: { status?: unknown; statusCode?: unknown };
  };
  if (typeof value.statusCode === "number") return value.statusCode;
  if (typeof value.response?.status === "number") return value.response.status;
  if (typeof value.response?.statusCode === "number") {
    return value.response.statusCode;
  }
  return undefined;
}

export function errorCategory(error: unknown): ErrorCategory {
  if (error instanceof RequestTimeoutError) return "timeout";
  if (error instanceof InvalidResponseError) return "invalid-response";

  const status = httpStatusCode(error);
  if (status === 429) return "throttled";
  if (status !== undefined && status >= 500 && status < 600) return "server";
  if (status !== undefined && status >= 400 && status < 500) return "client";

  if (error && typeof error === "object") {
    const code = (error as { code?: unknown }).code;
    if (
      typeof code === "string" &&
      new Set([
        "ECONNABORTED",
        "ECONNREFUSED",
        "ECONNRESET",
        "EHOSTUNREACH",
        "ENETDOWN",
        "ENETUNREACH",
        "ENOTFOUND",
        "EPIPE",
        "ETIMEDOUT",
        "EAI_AGAIN",
        "UND_ERR_CONNECT_TIMEOUT",
        "UND_ERR_HEADERS_TIMEOUT",
      ]).has(code)
    ) {
      return "network";
    }
  }
  return "unknown";
}

function iso(timestamp: number): string {
  return new Date(timestamp).toISOString();
}

async function writeJson(
  writer: AtomicWriter,
  path: string,
  document: StatusDocument | ReadyDocument,
): Promise<void> {
  await writer(path, `${JSON.stringify(document)}\n`, 0o644);
}

function extractOidcToken(value: unknown): string {
  if (typeof value !== "string" || value.trim() === "") {
    throw new InvalidResponseError();
  }
  return value;
}

async function requestWithTimeout(
  provider: OidcProvider,
  material: RefreshMaterial,
  signal: AbortSignal,
  timeoutMs: number,
): Promise<unknown> {
  if (signal.aborted) throw new ShutdownError();
  let timeout: NodeJS.Timeout | undefined;
  let abort: (() => void) | undefined;
  const timeoutPromise = new Promise<never>((_, reject) => {
    timeout = setTimeout(() => reject(new RequestTimeoutError()), timeoutMs);
  });
  const abortPromise = new Promise<never>((_, reject) => {
    abort = () => reject(new ShutdownError());
    signal.addEventListener("abort", abort, { once: true });
  });
  try {
    return await Promise.race([
      provider.createOidcToken(material),
      timeoutPromise,
      abortPromise,
    ]);
  } finally {
    if (timeout !== undefined) clearTimeout(timeout);
    if (abort !== undefined) signal.removeEventListener("abort", abort);
  }
}

async function stopped(
  material: RefreshMaterial,
  writer: AtomicWriter,
  now: () => number,
): Promise<number> {
  const timestamp = now();
  await writeJson(writer, material.statusPath, {
    state: "stopped",
    updatedAt: iso(timestamp),
    stoppedAt: iso(timestamp),
  });
  return 0;
}

async function unhealthy(
  material: RefreshMaterial,
  writer: AtomicWriter,
  now: () => number,
  timing: AssertionTiming,
  category: ErrorCategory,
): Promise<number> {
  await writeJson(writer, material.statusPath, {
    state: "unhealthy",
    updatedAt: iso(now()),
    assertionExpiresAt: iso(timing.expiresAt),
    errorCategory: category,
  });
  return 1;
}

/**
 * Run the refresh state machine until shutdown or until no valid assertion
 * remains. All diagnostics and status fields are fixed, sanitized values.
 */
export async function runRefresher(
  material: RefreshMaterial,
  signal: AbortSignal,
  dependencies: RuntimeDependencies = {},
): Promise<number> {
  const now = dependencies.now ?? Date.now;
  const sleep = dependencies.sleep ?? defaultSleep;
  const provider = dependencies.provider ?? new AzureDevOpsOidcProvider();
  const writer = dependencies.writeAtomic ?? writeAtomic;
  const report = dependencies.report ?? defaultReport;
  const requestTimeoutMs =
    dependencies.requestTimeoutMs ?? REQUEST_TIMEOUT_MS;

  let timing = assertionTiming(material.initialIdToken, now());
  let lastRefreshAt: number | undefined;
  let lastFailureCategory: ErrorCategory | undefined;

  try {
    await writeJson(writer, material.statusPath, {
      state: "starting",
      updatedAt: iso(now()),
    });
    await writer(material.tokenPath, material.initialIdToken, 0o644);
    if (timing.fallback) {
      report("assertion expiry is unavailable; using conservative timing");
    }
    if (timing.expiresAt <= now()) {
      return await unhealthy(
        material,
        writer,
        now,
        timing,
        "invalid-response",
      );
    }

    const readyAt = now();
    await writeJson(writer, material.statusPath, {
      state: "ready",
      updatedAt: iso(readyAt),
      assertionExpiresAt: iso(timing.expiresAt),
      nextRefreshAt: iso(timing.refreshAt),
    });
    await writeJson(writer, material.readyPath, {
      state: "ready",
      readyAt: iso(readyAt),
      assertionExpiresAt: iso(timing.expiresAt),
    });

    for (;;) {
      if (signal.aborted) return await stopped(material, writer, now);
      const waitMs = Math.max(0, timing.refreshAt - now());
      try {
        await sleep(waitMs, signal);
      } catch (error) {
        if (error instanceof ShutdownError || signal.aborted) {
          return await stopped(material, writer, now);
        }
        throw error;
      }
      if (signal.aborted) return await stopped(material, writer, now);

      let retryMs = INITIAL_RETRY_MS;
      for (;;) {
        if (signal.aborted) return await stopped(material, writer, now);
        const attemptAt = now();
        if (attemptAt >= timing.expiresAt) {
          return await unhealthy(
            material,
            writer,
            now,
            timing,
            lastFailureCategory ?? "timeout",
          );
        }

        await writeJson(writer, material.statusPath, {
          state: "refreshing",
          updatedAt: iso(attemptAt),
          assertionExpiresAt: iso(timing.expiresAt),
          lastRefreshAt:
            lastRefreshAt === undefined ? undefined : iso(lastRefreshAt),
        });

        try {
          const remainingMs = Math.max(1, timing.expiresAt - now());
          const response = await requestWithTimeout(
            provider,
            material,
            signal,
            Math.min(requestTimeoutMs, remainingMs),
          );
          const token = extractOidcToken(response);
          const refreshedAt = now();
          const nextTiming = assertionTiming(token, refreshedAt);
          if (!nextTiming.fallback && nextTiming.expiresAt <= refreshedAt) {
            throw new InvalidResponseError();
          }
          if (nextTiming.fallback) {
            report("refreshed assertion expiry is unavailable; using conservative timing");
          }
          if (signal.aborted) return await stopped(material, writer, now);

          await writer(material.tokenPath, token, 0o644);
          timing = nextTiming;
          lastRefreshAt = refreshedAt;
          lastFailureCategory = undefined;
          await writeJson(writer, material.statusPath, {
            state: "ready",
            updatedAt: iso(refreshedAt),
            assertionExpiresAt: iso(timing.expiresAt),
            nextRefreshAt: iso(timing.refreshAt),
            lastRefreshAt: iso(lastRefreshAt),
          });
          break;
        } catch (error) {
          if (error instanceof ShutdownError || signal.aborted) {
            return await stopped(material, writer, now);
          }
          const category = errorCategory(error);
          lastFailureCategory = category;
          const currentTime = now();
          if (currentTime >= timing.expiresAt) {
            return await unhealthy(
              material,
              writer,
              now,
              timing,
              category,
            );
          }

          const delay = Math.min(
            retryMs,
            MAX_RETRY_MS,
            timing.expiresAt - currentTime,
          );
          report(`refresh failed (${category}); retrying while assertion is valid`);
          await writeJson(writer, material.statusPath, {
            state: "refreshing",
            updatedAt: iso(currentTime),
            assertionExpiresAt: iso(timing.expiresAt),
            nextRefreshAt: iso(currentTime + delay),
            lastRefreshAt:
              lastRefreshAt === undefined ? undefined : iso(lastRefreshAt),
            errorCategory: category,
          });
          try {
            await sleep(delay, signal);
          } catch (sleepError) {
            if (sleepError instanceof ShutdownError || signal.aborted) {
              return await stopped(material, writer, now);
            }
            throw sleepError;
          }
          retryMs = Math.min(retryMs * 2, MAX_RETRY_MS);
        }
      }
    }
  } catch (error) {
    if (error instanceof ShutdownError || signal.aborted) {
      try {
        return await stopped(material, writer, now);
      } catch {
        report("failed to write stopped status (filesystem)");
        return 1;
      }
    }
    const category =
      error && typeof error === "object" && "code" in error
        ? "filesystem"
        : errorCategory(error);
    report(`sidecar failed (${category})`);
    try {
      return await unhealthy(material, writer, now, timing, category);
    } catch {
      report("failed to write unhealthy status (filesystem)");
      return 1;
    }
  }
}

/** Parse stdin, install signal handlers, and run the long-lived sidecar. */
export async function main(): Promise<number> {
  let material: RefreshMaterial;
  try {
    material = parseMaterial(await readOneJsonDocument());
  } catch (error) {
    defaultReport(
      error instanceof MaterialError
        ? `configuration error: ${error.message}`
        : "configuration error: cannot read material",
    );
    return 1;
  }

  const controller = new AbortController();
  const shutdown = (): void => controller.abort();
  process.once("SIGTERM", shutdown);
  process.once("SIGINT", shutdown);
  try {
    return await runRefresher(material, controller.signal);
  } finally {
    process.removeListener("SIGTERM", shutdown);
    process.removeListener("SIGINT", shutdown);
  }
}

if (
  typeof process !== "undefined" &&
  process.argv[1]?.endsWith("azure-wif-refresh.js")
) {
  void main().then(
    (code) => {
      process.exitCode = code;
    },
    () => {
      defaultReport("sidecar failed (unknown)");
      process.exitCode = 1;
    },
  );
}
