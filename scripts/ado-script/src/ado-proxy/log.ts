/**
 * Sanitized decision log.
 *
 * This stream is copied into the Agent's published artifacts and read by
 * `ado-aw audit`, so it is written under the assumption that the agent will
 * read it. It therefore records *shapes and outcomes*, never content: no
 * headers, no bodies, no query values, no URLs beyond the normalized operation
 * id and the scope identifiers the policy already pinned.
 */
import { appendFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";

/** Schema version, so `ado-aw audit` can evolve its reader independently. */
export const DECISION_LOG_SCHEMA = "ado-aw/ado-proxy-decisions/v1";

export interface DecisionRecord {
  /** ISO-8601 timestamp. */
  readonly ts: string;
  /** Correlates the request across the allow decision and the response. */
  readonly request_id: string;
  readonly host: string;
  readonly method: string;
  /** Catalog operation id, when one matched. */
  readonly operation?: string;
  readonly decision: "allow" | "deny" | "error";
  /** Machine-readable denial reason; absent on allow. */
  readonly reason?: string;
  /** Short human-readable detail. Never contains request content. */
  readonly detail?: string;
  /** Upstream status class (`2xx`, `4xx`, …), not the exact code. */
  readonly upstream_status_class?: string;
  readonly latency_ms?: number;
  readonly response_bytes?: number;
  /** Credential headers the client supplied and the proxy stripped. */
  readonly stripped_credentials?: readonly string[];
}

/**
 * Append-only JSONL writer.
 *
 * Failures to write are swallowed after the first report: losing audit lines is
 * bad, but killing the proxy — and therefore the agent's only route to Azure
 * DevOps — because a log volume filled up would be worse.
 */
export class DecisionLog {
  readonly #path: string | undefined;
  #warned = false;

  constructor(logDir: string | undefined) {
    if (logDir === undefined) {
      this.#path = undefined;
      return;
    }
    try {
      mkdirSync(logDir, { recursive: true });
      this.#path = join(logDir, "ado-proxy-decisions.jsonl");
      appendFileSync(this.#path, `${JSON.stringify({ schema: DECISION_LOG_SCHEMA })}\n`);
    } catch (error) {
      process.stderr.write(
        `[ado-proxy] decision log disabled: ${(error as Error).message}\n`,
      );
      this.#path = undefined;
    }
  }

  write(record: DecisionRecord): void {
    if (this.#path === undefined) return;
    try {
      appendFileSync(this.#path, `${JSON.stringify(record)}\n`);
    } catch (error) {
      if (!this.#warned) {
        this.#warned = true;
        process.stderr.write(
          `[ado-proxy] decision log write failed: ${(error as Error).message}\n`,
        );
      }
    }
  }
}

/** Bucket an HTTP status into its class, so exact upstream codes never leak. */
export function statusClass(status: number): string {
  return `${Math.floor(status / 100)}xx`;
}
