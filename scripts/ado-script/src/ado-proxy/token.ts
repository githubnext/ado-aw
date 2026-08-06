/**
 * Access to the Azure DevOps bearer.
 *
 * The token arrives on **stdin**, in the same stream as the interception
 * certificates, and is held in memory for the life of the process. It is
 * deliberately not passed in argv or the environment — both are readable from
 * the process table and from `/proc` — and deliberately not written to a file.
 *
 * A file would be the obvious choice, and is unsafe here: AWF mounts the
 * runner's `/tmp` into the agent at both `/tmp` and `/host/tmp`, which is how
 * AWF installs its own `gh` wrapper. Anything the engine wrote to a runner path
 * would therefore be readable by the very agent the credential is being hidden
 * from.
 *
 * **No rotation.** A stdin-delivered token cannot be replaced without
 * restarting the process, so the run is bounded by the token's lifetime. The
 * compiler enforces that bound up front by refusing to compile a workflow whose
 * `timeout-minutes` could outlive the token, which turns a mid-run failure —
 * where the agent would see opaque `502`s — into a compile error naming the
 * cause. Rotation needs a different delivery mechanism (a private volume, or
 * `docker cp` into the running container) and is tracked separately.
 */

export class TokenError extends Error {}

/**
 * The bearer, held in memory.
 *
 * A class rather than a bare string so the token has a single accessor to
 * audit, and so a future rotating implementation can replace it without
 * touching callers.
 */
export class TokenSource {
  readonly #value: string;

  constructor(value: string) {
    const trimmed = value.trim();
    if (trimmed === "") {
      throw new TokenError("the Azure DevOps bearer is empty");
    }
    this.#value = trimmed;
  }

  /**
   * Return the bearer.
   *
   * Infallible by construction: an empty or absent token is rejected at
   * startup, so no request path can forward unauthenticated. That matters
   * because Azure DevOps answers an unauthenticated request with a sign-in
   * page, which a client could mistake for data.
   */
  read(): string {
    return this.#value;
  }
}

/** Format the bearer for the `Authorization` header. */
export function bearerHeader(token: string): string {
  return `Bearer ${token}`;
}
