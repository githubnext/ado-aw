/**
 * `ado-proxy.js resolve-feeds` — fill feed, project, and view GUIDs into the
 * policy before the proxy starts.
 *
 * Package clients address a feed by name *or* by id, and Azure Artifacts' own
 * service index hands out id-based URLs, so the package authorizer must know
 * both. The compiler only knows names; this step asks Azure Artifacts for the
 * ids with the same package credential the proxy will later use, and writes
 * them back into the policy file.
 *
 * It doubles as a preflight. Running on the **trusted host**, before the agent
 * exists, it turns "the pipeline identity cannot read this feed" from a
 * baffling mid-run `502` into an Azure Pipelines error naming the feed, the
 * identity, and the role to grant.
 *
 * Credential custody matches the proxy: the token is read from **stdin** only,
 * never argv or the environment, and is never printed — every message is built
 * from policy names and HTTP status codes, and redacted as a final guard.
 */
import { randomBytes } from "node:crypto";
import { readFileSync, renameSync, statSync, unlinkSync, writeFileSync } from "node:fs";
import { basename, dirname, join } from "node:path";

import { ConfigError, isCanonicalGuid, parsePolicy, type PackageFeedGrant } from "./config.js";

/** Injectable effects, so the resolver is unit-testable without a network. */
export interface ResolveFeedsDeps {
  readonly fetch: typeof fetch;
  /** Read the whole of stdin. */
  readonly readStdin: () => string;
  /** Write one line (without trailing newline) to stderr. */
  readonly stderr: (line: string) => void;
  /** Per-request timeout. Defaults to {@link REQUEST_TIMEOUT_MS}. */
  readonly timeoutMs?: number;
}

/** Long enough for a slow Azure DevOps response, short enough to fail a hung one. */
const REQUEST_TIMEOUT_MS = 30_000;

const API_VERSION = "7.1";

/** Outcome of one GET against Azure DevOps. */
type Fetched =
  | { readonly kind: "ok"; readonly body: unknown }
  | { readonly kind: "status"; readonly status: number }
  | { readonly kind: "network"; readonly message: string };

function readPolicyFileOption(argv: readonly string[]): string | undefined {
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--policy-file") return argv[index + 1];
    if (arg?.startsWith("--policy-file=")) return arg.slice("--policy-file=".length);
  }
  return undefined;
}

/**
 * Escape a value for an Azure Pipelines logging command.
 *
 * Without this a newline in a feed name would end the `##vso` command early
 * and let the rest of the line be read as a fresh command.
 */
function escapeLoggingCommand(value: string): string {
  return value
    .replace(/%/g, "%AZP25")
    .replace(/\r/g, "%0D")
    .replace(/\n/g, "%0A");
}

/** Strip anything that could forge a log line or a terminal escape. */
function printable(value: string, max = 200): string {
  // eslint-disable-next-line no-control-regex
  return value.replace(/[\u0000-\u001f\u007f]/g, " ").slice(0, max);
}

function feedLabel(grant: PackageFeedGrant): string {
  return [grant.organization, grant.project, grant.feed]
    .filter((part): part is string => part !== undefined)
    .join("/");
}

/** `https://feeds.dev.azure.com/{org}[/{project}]/_apis/packaging/feeds/` */
function feedsBase(grant: PackageFeedGrant): string {
  const project = grant.project === undefined ? "" : `${encodeURIComponent(grant.project)}/`;
  return (
    `https://feeds.dev.azure.com/${encodeURIComponent(grant.organization)}/` +
    `${project}_apis/packaging/feeds/`
  );
}

function asRecord(value: unknown): Record<string, unknown> | undefined {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined;
}

/** Read a GUID-shaped string field from an API response, or `undefined`. */
function guidField(source: unknown, key: string): string | undefined {
  const value = asRecord(source)?.[key];
  return typeof value === "string" && isCanonicalGuid(value) ? value : undefined;
}

/**
 * Resolve every feed grant's ids and rewrite the policy file.
 *
 * Returns the process exit code: `0` on success (including a policy with no
 * `packages` section, which needs nothing), `1` on any failure. A failure
 * leaves the policy file untouched.
 */
export async function resolveFeeds(
  argv: readonly string[],
  deps: ResolveFeedsDeps,
): Promise<number> {
  let token = "";
  // Last line of defence: no message may carry the credential, whatever an
  // upstream error or a future edit puts into it.
  const redact = (text: string): string =>
    token === "" ? text : text.split(token).join("***");
  const say = (line: string): void => deps.stderr(redact(line));
  const error = (summary: string, ...explanation: string[]): number => {
    say(`##vso[task.logissue type=error]${escapeLoggingCommand(redact(summary))}`);
    for (const line of explanation) say(`[ado-proxy] ${line}`);
    return 1;
  };

  const policyFile = readPolicyFileOption(argv);
  if (policyFile === undefined || policyFile.trim() === "") {
    return error("ado-proxy resolve-feeds: missing required option --policy-file");
  }

  let raw: string;
  let policy;
  try {
    raw = readFileSync(policyFile, "utf8");
    policy = parsePolicy(raw);
  } catch (caught) {
    const message = caught instanceof ConfigError ? caught.message : (caught as Error).message;
    return error(`ado-proxy resolve-feeds: cannot load the policy: ${printable(message, 500)}`);
  }

  const packages = policy.packages;
  if (packages === undefined) {
    say("[ado-proxy] resolve-feeds: the policy grants no package feeds; nothing to resolve");
    return 0;
  }

  token = deps.readStdin().trim();
  if (token === "") {
    return error(
      "ado-proxy resolve-feeds: no package credential on stdin; the host step must pipe it in",
    );
  }

  const get = async (url: string): Promise<Fetched> => {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), deps.timeoutMs ?? REQUEST_TIMEOUT_MS);
    timer.unref?.();
    try {
      const response = await deps.fetch(url, {
        method: "GET",
        headers: {
          authorization: `Bearer ${token}`,
          accept: "application/json",
          // Ask for a 401 rather than a 203 sign-in page on a bad credential.
          "x-tfs-fedauthredirect": "Suppress",
        },
        // A redirect would carry the bearer to wherever it pointed.
        redirect: "manual",
        signal: controller.signal,
      });
      // Exactly 200: `response.ok` also admits 203, which is Azure DevOps'
      // sign-in page masquerading as success.
      if (response.status !== 200) {
        await response.body?.cancel().catch(() => undefined);
        return { kind: "status", status: response.status };
      }
      try {
        return { kind: "ok", body: await response.json() };
      } catch {
        return { kind: "network", message: "the response was not valid JSON" };
      }
    } catch (caught) {
      const message =
        (caught as Error).name === "AbortError"
          ? `no response within ${(deps.timeoutMs ?? REQUEST_TIMEOUT_MS) / 1000}s`
          : (caught as Error).message;
      return { kind: "network", message };
    } finally {
      clearTimeout(timer);
    }
  };

  /** Best-effort: the display name of the identity the credential belongs to. */
  const identityName = async (organization: string): Promise<string | undefined> => {
    try {
      const outcome = await get(
        `https://dev.azure.com/${encodeURIComponent(organization)}/_apis/connectionData`,
      );
      if (outcome.kind !== "ok") return undefined;
      const name = asRecord(asRecord(outcome.body)?.authenticatedUser)?.providerDisplayName;
      return typeof name === "string" && name.trim() !== "" ? printable(name.trim()) : undefined;
    } catch {
      return undefined;
    }
  };

  const failure = async (
    grant: PackageFeedGrant,
    what: string,
    outcome: Exclude<Fetched, { kind: "ok" }>,
  ): Promise<number> => {
    const label = printable(feedLabel(grant));
    const scope = printable(
      grant.project === undefined
        ? `organization ${grant.organization}`
        : `organization ${grant.organization}, project ${grant.project}`,
    );
    const identity = await identityName(grant.organization);
    const who =
      identity === undefined ? "The pipeline identity" : `The pipeline identity "${identity}"`;
    const role =
      "Grant the pipeline identity the Feed Reader role on the feed " +
      "(Artifacts -> feed settings -> Permissions), or Feed and Upstream Reader " +
      "when the feed is configured with upstream: allow.";

    if (outcome.kind === "network") {
      return error(
        `ado-proxy: cannot reach Azure Artifacts to resolve ${what} for feed ${label}: ` +
          printable(outcome.message),
        `Feed ${printable(grant.feed)} in ${scope} could not be resolved because Azure ` +
          "Artifacts did not answer. Check the agent's network access to feeds.dev.azure.com.",
      );
    }

    const status = outcome.status;
    const summary = `ado-proxy: cannot resolve ${what} for Azure Artifacts feed ${label} (HTTP ${status})`;
    if (status === 401 || status === 203) {
      return error(
        summary,
        `${who} was not authenticated by Azure Artifacts when reading feed ` +
          `${printable(grant.feed)} in ${scope}.`,
        `The package credential is missing, expired, or not valid for this organization. ${role}`,
      );
    }
    if (status === 403) {
      return error(
        summary,
        `${who} is not allowed to read feed ${printable(grant.feed)} in ${scope}.`,
        role,
      );
    }
    if (status === 404) {
      return error(
        summary,
        `Feed ${printable(grant.feed)}${grant.view === undefined ? "" : `@${printable(grant.view)}`} ` +
          `was not found in ${scope}: check the feed/project name and that the identity ` +
          "can see the project.",
        `Azure Artifacts also answers 404 for a feed the identity cannot see. ${role}`,
      );
    }
    return error(
      summary,
      `Azure Artifacts answered HTTP ${status} when reading feed ${printable(grant.feed)} in ${scope}.`,
    );
  };

  // Mutate the raw document rather than re-serializing the parsed policy, so
  // every field this step does not own is written back exactly as compiled.
  const document = JSON.parse(raw) as Record<string, unknown>;
  const feedDocuments = (asRecord(document.packages)?.feeds ?? []) as Record<string, unknown>[];

  for (const [index, grant] of packages.feeds.entries()) {
    const target = feedDocuments[index];
    if (target === undefined) return error("ado-proxy resolve-feeds: policy feeds changed while reading");

    const feedOutcome = await get(
      `${feedsBase(grant)}${encodeURIComponent(grant.feed)}?api-version=${API_VERSION}`,
    );
    if (feedOutcome.kind !== "ok") return failure(grant, "the feed id", feedOutcome);

    const feedId = guidField(feedOutcome.body, "id");
    if (feedId === undefined) {
      return error(
        `ado-proxy: Azure Artifacts returned no feed id for ${printable(feedLabel(grant))}`,
      );
    }
    target.feed_id = feedId;

    if (grant.project !== undefined) {
      // Only a project-scoped grant records a project id; adding one to an
      // organization-scoped grant would change which URLs it matches.
      const projectId = guidField(asRecord(feedOutcome.body)?.project, "id");
      if (projectId !== undefined) target.project_id = projectId;
    }

    if (grant.view !== undefined) {
      const viewOutcome = await get(
        `${feedsBase(grant)}${feedId}/views/${encodeURIComponent(grant.view)}` +
          `?api-version=${API_VERSION}`,
      );
      if (viewOutcome.kind !== "ok") return failure(grant, `view ${grant.view}`, viewOutcome);
      const viewId = guidField(viewOutcome.body, "id");
      if (viewId === undefined) {
        return error(
          `ado-proxy: Azure Artifacts returned no view id for ` +
            `${printable(feedLabel(grant))}@${printable(grant.view)}`,
        );
      }
      target.view_id = viewId;
    }
  }

  const serialized = `${JSON.stringify(document, null, 2)}\n`;
  try {
    // Re-validate before writing, so this step can never hand the proxy a
    // policy it would refuse to start with.
    parsePolicy(serialized);
    writeAtomically(policyFile, serialized);
  } catch (caught) {
    return error(
      `ado-proxy resolve-feeds: cannot write the resolved policy: ${printable((caught as Error).message, 500)}`,
    );
  }

  say(`[ado-proxy] resolve-feeds: resolved ${packages.feeds.length} package feed(s)`);
  return 0;
}

/**
 * Replace `path` with `contents` atomically.
 *
 * Written beside the target and renamed over it, so a crash mid-write leaves
 * either the old policy or the new one — never a truncated file the proxy
 * would refuse, or worse, partially parse.
 */
function writeAtomically(path: string, contents: string): void {
  const mode = statSync(path).mode & 0o777;
  const temporary = join(
    dirname(path),
    `.${basename(path)}.${process.pid}.${randomBytes(6).toString("hex")}.tmp`,
  );
  try {
    writeFileSync(temporary, contents, { mode, flag: "wx" });
    renameSync(temporary, path);
  } catch (caught) {
    try {
      unlinkSync(temporary);
    } catch {
      // Nothing to clean up.
    }
    throw caught;
  }
}

/** Entry point used by `index.ts`, wired to the real process. */
export function runResolveFeeds(argv: readonly string[]): Promise<number> {
  return resolveFeeds(argv, {
    fetch: globalThis.fetch,
    readStdin: () => readFileSync(0, "utf8"),
    stderr: (line) => process.stderr.write(`${line}\n`),
  });
}

export const INTERNAL = { escapeLoggingCommand };
