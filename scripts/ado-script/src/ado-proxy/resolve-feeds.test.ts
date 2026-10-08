/**
 * Tests for the trusted-host feed resolver.
 *
 * The resolver holds the package credential, so beyond "fills in the right
 * ids" these pin its custody rules: the token comes only from stdin, is sent
 * only as a bearer to Azure DevOps, and never appears in anything it prints —
 * including the actionable error an operator sees when the pipeline identity
 * lacks feed access.
 */
import { mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { CATALOG_SCHEMA_VERSION } from "./catalog.js";
import { parsePolicy } from "./config.js";
import { INTERNAL, resolveFeeds, type ResolveFeedsDeps } from "./resolve-feeds.js";

const TOKEN = "package-canary-5e1f9b";
const PROJECT_ID = "11111111-1111-1111-1111-111111111111";
const FEED_ID = "22222222-2222-2222-2222-222222222222";
const VIEW_ID = "33333333-3333-3333-3333-333333333333";

const BASE_POLICY = {
  catalog_version: CATALOG_SCHEMA_VERSION,
  organization: "contoso",
  project: "Engineering",
  capabilities: [],
  protected_hosts: ["dev.azure.com", "app.vssps.visualstudio.com"],
  allowed_resource_areas: [],
};

interface Recorded {
  readonly url: string;
  readonly headers: Record<string, string>;
  readonly redirect: string | undefined;
}

type Route = (url: string) => Response | Promise<Response>;

function harness(route: Route, stdin = `${TOKEN}\n`) {
  const requests: Recorded[] = [];
  const stderr: string[] = [];
  let stdinReads = 0;
  const deps: ResolveFeedsDeps = {
    fetch: (async (input: string | URL | Request, init?: RequestInit) => {
      const url = String(input);
      requests.push({
        url,
        headers: { ...(init?.headers as Record<string, string>) },
        redirect: init?.redirect,
      });
      return route(url);
    }) as typeof fetch,
    readStdin: () => {
      stdinReads += 1;
      return stdin;
    },
    stderr: (line) => stderr.push(line),
  };
  return { deps, requests, stderr, stdinReads: () => stdinReads };
}

const json = (body: unknown, status = 200): Response =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });

let directory: string;
let policyFile: string;

function writePolicy(document: Record<string, unknown>): string {
  const raw = JSON.stringify(document);
  writeFileSync(policyFile, raw);
  return raw;
}

beforeEach(() => {
  directory = mkdtempSync(join(tmpdir(), "ado-proxy-resolve-"));
  policyFile = join(directory, "policy.json");
});

afterEach(() => {
  rmSync(directory, { recursive: true, force: true });
});

describe("resolveFeeds", () => {
  it("fills feed, project, and view ids for a project-scoped feed with a view", async () => {
    writePolicy({
      ...BASE_POLICY,
      packages: {
        hosts: ["pkgs.dev.azure.com"],
        feeds: [
          {
            organization: "contoso",
            project: "My Project",
            feed: "internal",
            view: "Release",
            protocols: ["npm"],
          },
        ],
      },
    });
    const { deps, requests, stderr } = harness((url) => {
      if (url.includes("/views/")) return json({ id: VIEW_ID, name: "Release" });
      return json({ id: FEED_ID, name: "internal", project: { id: PROJECT_ID, name: "My Project" } });
    });

    expect(await resolveFeeds(["--policy-file", policyFile], deps)).toBe(0);

    expect(requests.map((request) => request.url)).toEqual([
      "https://feeds.dev.azure.com/contoso/My%20Project/_apis/packaging/feeds/internal?api-version=7.1",
      `https://feeds.dev.azure.com/contoso/My%20Project/_apis/packaging/feeds/${FEED_ID}/views/Release?api-version=7.1`,
    ]);
    for (const request of requests) {
      expect(request.headers.authorization).toBe(`Bearer ${TOKEN}`);
      expect(request.headers.accept).toBe("application/json");
      // A followed redirect would carry the bearer wherever it pointed.
      expect(request.redirect).toBe("manual");
    }

    const written = readFileSync(policyFile, "utf8");
    expect(written.endsWith("}\n")).toBe(true);
    expect(written).toContain('\n  "packages": {');
    const policy = parsePolicy(written);
    expect(policy.packages?.feeds[0]).toMatchObject({
      feed_id: FEED_ID,
      project_id: PROJECT_ID,
      view_id: VIEW_ID,
    });
    // Fields the resolver does not own are written back as compiled.
    expect(policy.organization).toBe("contoso");
    expect(policy.capabilities).toEqual([]);
    expect(stderr.join("\n")).not.toContain(TOKEN);
    // The atomic write leaves no temporary file behind.
    expect(readdirSync(directory)).toEqual(["policy.json"]);
  });

  it("fills only the feed id for an organization-scoped feed without a view", async () => {
    writePolicy({
      ...BASE_POLICY,
      packages: {
        hosts: ["pkgs.dev.azure.com"],
        feeds: [{ organization: "contoso", feed: "shared", protocols: ["pypi"] }],
      },
    });
    const { deps, requests } = harness(() =>
      json({ id: FEED_ID, project: { id: PROJECT_ID } }),
    );

    expect(await resolveFeeds([`--policy-file=${policyFile}`], deps)).toBe(0);

    expect(requests.map((request) => request.url)).toEqual([
      "https://feeds.dev.azure.com/contoso/_apis/packaging/feeds/shared?api-version=7.1",
    ]);
    const feed = parsePolicy(readFileSync(policyFile, "utf8")).packages?.feeds[0];
    expect(feed?.feed_id).toBe(FEED_ID);
    // A project id on an organization-scoped grant would change what it matches.
    expect(feed?.project_id).toBeUndefined();
    expect(feed?.view_id).toBeUndefined();
  });

  it("explains a 403 with the feed, the identity, and the role to grant", async () => {
    const raw = writePolicy({
      ...BASE_POLICY,
      packages: {
        hosts: ["pkgs.dev.azure.com"],
        feeds: [{ organization: "contoso", project: "Engineering", feed: "internal", protocols: ["nuget"] }],
      },
    });
    const { deps, stderr } = harness((url) => {
      if (url === "https://dev.azure.com/contoso/_apis/connectionData") {
        return json({ authenticatedUser: { providerDisplayName: "Engineering Build Service (contoso)" } });
      }
      return new Response(`denied for ${TOKEN}`, { status: 403 });
    });

    expect(await resolveFeeds(["--policy-file", policyFile], deps)).toBe(1);

    const output = stderr.join("\n");
    expect(stderr[0]).toMatch(/^##vso\[task\.logissue type=error\]/);
    expect(output).toContain("contoso/Engineering/internal");
    expect(output).toContain("HTTP 403");
    expect(output).toContain("Engineering Build Service (contoso)");
    expect(output).toContain("Feed Reader");
    expect(output).toContain("Feed and Upstream Reader");
    expect(output).not.toContain(TOKEN);
    // A failure leaves the policy untouched.
    expect(readFileSync(policyFile, "utf8")).toBe(raw);
  });

  it("explains a 404 as a naming or visibility problem", async () => {
    writePolicy({
      ...BASE_POLICY,
      packages: {
        hosts: ["pkgs.dev.azure.com"],
        feeds: [{ organization: "contoso", feed: "typo", view: "Release", protocols: ["npm"] }],
      },
    });
    const { deps, stderr } = harness(() => new Response("", { status: 404 }));

    expect(await resolveFeeds(["--policy-file", policyFile], deps)).toBe(1);

    const output = stderr.join("\n");
    expect(output).toContain("HTTP 404");
    expect(output).toContain("typo@Release");
    expect(output).toContain("check the feed/project name");
    expect(output).not.toContain(TOKEN);
  });

  it("treats a 401 or 203 sign-in page as unauthenticated, without an identity name", async () => {
    for (const status of [401, 203]) {
      writePolicy({
        ...BASE_POLICY,
        packages: {
          hosts: ["pkgs.dev.azure.com"],
          feeds: [{ organization: "contoso", feed: "shared", protocols: ["npm"] }],
        },
      });
      const { deps, stderr } = harness(() => new Response("<html>sign in</html>", { status }));
      expect(await resolveFeeds(["--policy-file", policyFile], deps)).toBe(1);
      const output = stderr.join("\n");
      expect(output).toContain(`HTTP ${status}`);
      expect(output).toContain("not authenticated");
      expect(output).toContain('The pipeline identity was');
      expect(output).not.toContain(TOKEN);
    }
  });

  it("reports a view that cannot be resolved", async () => {
    writePolicy({
      ...BASE_POLICY,
      packages: {
        hosts: ["pkgs.dev.azure.com"],
        feeds: [{ organization: "contoso", feed: "shared", view: "Nope", protocols: ["npm"] }],
      },
    });
    const { deps, stderr } = harness((url) =>
      url.includes("/views/") ? new Response("", { status: 404 }) : json({ id: FEED_ID }),
    );
    expect(await resolveFeeds(["--policy-file", policyFile], deps)).toBe(1);
    expect(stderr.join("\n")).toContain("view Nope");
  });

  it("reports a network failure without leaking the token", async () => {
    writePolicy({
      ...BASE_POLICY,
      packages: {
        hosts: ["pkgs.dev.azure.com"],
        feeds: [{ organization: "contoso", feed: "shared", protocols: ["npm"] }],
      },
    });
    const { deps, stderr } = harness(() => {
      throw new TypeError(`fetch failed (${TOKEN})`);
    });
    expect(await resolveFeeds(["--policy-file", policyFile], deps)).toBe(1);
    const output = stderr.join("\n");
    expect(output).toContain("cannot reach Azure Artifacts");
    expect(output).not.toContain(TOKEN);
  });

  it("refuses a response without a GUID id", async () => {
    writePolicy({
      ...BASE_POLICY,
      packages: {
        hosts: ["pkgs.dev.azure.com"],
        feeds: [{ organization: "contoso", feed: "shared", protocols: ["npm"] }],
      },
    });
    const { deps } = harness(() => json({ id: "not-a-guid" }));
    expect(await resolveFeeds(["--policy-file", policyFile], deps)).toBe(1);
  });

  it("does nothing for a policy with no packages section", async () => {
    const raw = writePolicy({ ...BASE_POLICY, capabilities: ["core"] });
    const { deps, requests, stdinReads } = harness(() => json({}));

    expect(await resolveFeeds(["--policy-file", policyFile], deps)).toBe(0);
    expect(requests).toHaveLength(0);
    expect(stdinReads()).toBe(0);
    expect(readFileSync(policyFile, "utf8")).toBe(raw);
  });

  it("refuses an empty stdin and a missing --policy-file", async () => {
    writePolicy({
      ...BASE_POLICY,
      packages: {
        hosts: ["pkgs.dev.azure.com"],
        feeds: [{ organization: "contoso", feed: "shared", protocols: ["npm"] }],
      },
    });
    const empty = harness(() => json({ id: FEED_ID }), " \n");
    expect(await resolveFeeds(["--policy-file", policyFile], empty.deps)).toBe(1);
    expect(empty.requests).toHaveLength(0);
    expect(empty.stderr.join("\n")).toContain("no package credential on stdin");

    const missing = harness(() => json({ id: FEED_ID }));
    expect(await resolveFeeds([], missing.deps)).toBe(1);
    expect(missing.stderr.join("\n")).toContain("--policy-file");
  });

  it("refuses an invalid policy file", async () => {
    writeFileSync(policyFile, "{not json");
    const { deps } = harness(() => json({}));
    expect(await resolveFeeds(["--policy-file", policyFile], deps)).toBe(1);
  });
});

describe("escapeLoggingCommand", () => {
  it("prevents a value from ending the logging command early", () => {
    expect(INTERNAL.escapeLoggingCommand("a\r\n##vso[task.setvariable]x%")).toBe(
      "a%0D%0A##vso[task.setvariable]x%AZP25",
    );
  });
});
