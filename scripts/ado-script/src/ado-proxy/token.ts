/**
 * Access to the Azure DevOps bearer.
 *
 * The token lives in a file the trusted host task rotates and mounts read-only
 * into this container. It is deliberately *not* passed in argv or the
 * environment: both are readable from the process table and from `/proc`, and
 * neither can be rotated without restarting the proxy.
 *
 * Reads are cached on the file's mtime and size so the hot path does not stat-
 * and-read per request, while a rotation still takes effect on the next
 * request rather than at some later refresh tick.
 */
import { readFileSync, statSync } from "node:fs";

export class TokenError extends Error {}

interface CachedToken {
  readonly mtimeMs: number;
  readonly size: number;
  readonly value: string;
}

export class TokenSource {
  readonly #path: string;
  #cached: CachedToken | undefined;

  constructor(path: string) {
    this.#path = path;
  }

  /**
   * Return the current bearer.
   *
   * Throws {@link TokenError} when the file is missing, unreadable, or empty.
   * Callers must translate that into an infrastructure failure — never into a
   * request forwarded without credentials, which Azure DevOps would answer
   * with a sign-in page the agent could mistake for data.
   */
  read(): string {
    let stats: ReturnType<typeof statSync>;
    try {
      stats = statSync(this.#path);
    } catch (error) {
      this.#cached = undefined;
      throw new TokenError(
        `token file ${this.#path} is unavailable: ${(error as Error).message}`,
      );
    }

    const cached = this.#cached;
    if (
      cached !== undefined &&
      cached.mtimeMs === stats.mtimeMs &&
      cached.size === stats.size
    ) {
      return cached.value;
    }

    let raw: string;
    try {
      raw = readFileSync(this.#path, "utf8");
    } catch (error) {
      this.#cached = undefined;
      throw new TokenError(
        `token file ${this.#path} is unreadable: ${(error as Error).message}`,
      );
    }

    const value = raw.trim();
    if (value === "") {
      this.#cached = undefined;
      throw new TokenError(`token file ${this.#path} is empty`);
    }

    this.#cached = { mtimeMs: stats.mtimeMs, size: stats.size, value };
    return value;
  }

  /** Drop the cache. Used by tests and after an upstream 401. */
  invalidate(): void {
    this.#cached = undefined;
  }
}

/**
 * Build the `Authorization` header value for an authorized request.
 *
 * Azure DevOps accepts the AAD access token as a bearer; the sentinel PAT the
 * agent may have supplied was already stripped by {@link sanitizeRequestHeaders}.
 */
export function bearerHeader(token: string): string {
  return `Bearer ${token}`;
}
