import {
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it, vi } from "vitest";

import {
  assertionTiming,
  parseJwtExpiryMs,
  parseMaterial,
  requestOidcToken,
  runRefresher,
  writeAtomic,
  type AtomicWriter,
  type RefreshMaterial,
  type StatusDocument,
} from "../index.js";

const INITIAL_TOKEN = "initial.secret.token";
const SYSTEM_TOKEN = "system.secret.token";

function jwt(expSeconds: number): string {
  const header = Buffer.from('{"alg":"none"}').toString("base64url");
  const payload = Buffer.from(JSON.stringify({ exp: expSeconds })).toString(
    "base64url",
  );
  return `${header}.${payload}.signature`;
}

function material(overrides: Partial<RefreshMaterial> = {}): RefreshMaterial {
  return {
    initialIdToken: INITIAL_TOKEN,
    systemAccessToken: SYSTEM_TOKEN,
    oidcRequestUri:
      "https://dev.azure.com/example/_apis/distributedtask/hubs/build/plans/plan/jobs/job/oidctoken",
    serviceConnectionId: "11111111-2222-3333-4444-555555555555",
    tokenPath: "/state/token",
    readyPath: "/state/ready.json",
    statusPath: "/state/status.json",
    ...overrides,
  };
}

function recordingWriter() {
  const files = new Map<string, { content: string; mode: number }>();
  const writes: Array<{ path: string; content: string; mode: number }> = [];
  const writer: AtomicWriter = async (path, content, mode) => {
    writes.push({ path, content, mode });
    files.set(path, { content, mode });
  };
  return { files, writes, writer };
}

function statusDocuments(
  writes: Array<{ path: string; content: string }>,
  path = "/state/status.json",
): StatusDocument[] {
  return writes
    .filter((write) => write.path === path)
    .map((write) => JSON.parse(write.content) as StatusDocument);
}

const tempDirs: string[] = [];

afterEach(() => {
  vi.restoreAllMocks();
  for (const directory of tempDirs.splice(0)) {
    rmSync(directory, { recursive: true, force: true });
  }
});

describe("material and expiry parsing", () => {
  it("accepts the closed material schema and rejects unknown or empty fields", () => {
    expect(parseMaterial(JSON.stringify(material()))).toEqual(material());
    expect(() =>
      parseMaterial(JSON.stringify({ ...material(), extra: "nope" })),
    ).toThrow(/unknown fields/);
    expect(() =>
      parseMaterial(JSON.stringify({ ...material(), serviceConnectionId: "" })),
    ).toThrow(/serviceConnectionId/);
    expect(() =>
      parseMaterial(
        JSON.stringify({ ...material(), serviceConnectionId: "not-a-guid" }),
      ),
    ).toThrow(/GUID/);
  });

  it("parses a JWT exp without verifying the signature", () => {
    expect(parseJwtExpiryMs(jwt(1_700_000_123))).toBe(1_700_000_123_000);
    expect(parseJwtExpiryMs("not-a-jwt")).toBeUndefined();
    expect(parseJwtExpiryMs("a.e30.c")).toBeUndefined();
    expect(parseJwtExpiryMs("a.WyJub3QiLCJhbiIsIm9iamVjdCJd.c")).toBeUndefined();
  });

  it("refreshes 60 seconds before exp and falls back to four minutes", () => {
    const now = 1_700_000_000_000;
    expect(assertionTiming(jwt(now / 1000 + 300), now)).toEqual({
      expiresAt: now + 300_000,
      refreshAt: now + 240_000,
      fallback: false,
    });
    expect(assertionTiming("malformed", now)).toEqual({
      expiresAt: now + 300_000,
      refreshAt: now + 240_000,
      fallback: true,
    });
  });
});

describe("atomic publication", () => {
  it("atomically replaces the assertion with mode 0644", async () => {
    const directory = mkdtempSync(join(tmpdir(), "ado-aw-wif-"));
    tempDirs.push(directory);
    const tokenPath = join(directory, "token");
    writeFileSync(tokenPath, "old", "utf8");

    await writeAtomic(tokenPath, INITIAL_TOKEN, 0o644);

    expect(readFileSync(tokenPath, "utf8")).toBe(INITIAL_TOKEN);
    if (process.platform !== "win32") {
      expect(statSync(tokenPath).mode & 0o777).toBe(0o644);
    }
    expect(readdirSync(directory)).toEqual(["token"]);
  });
});

describe("OIDC refresh request", () => {
  it("posts to the supplied endpoint with the bearer and service connection GUID", async () => {
    const fetchFn = vi.fn().mockResolvedValue({
      ok: true,
      status: 200,
      json: async () => ({ oidcToken: "refreshed.assertion.value" }),
    });
    const value = material({
      oidcRequestUri: "https://example.test/oidc",
      serviceConnectionId: "id with spaces",
    });

    await expect(requestOidcToken(value, fetchFn)).resolves.toBe(
      "refreshed.assertion.value",
    );
    expect(fetchFn).toHaveBeenCalledWith(
      "https://example.test/oidc?api-version=7.1&serviceConnectionId=id%20with%20spaces",
      {
        method: "POST",
        headers: {
          Authorization: `Bearer ${SYSTEM_TOKEN}`,
          "Content-Type": "application/json",
          "X-TFS-FedAuthRedirect": "Suppress",
        },
        body: "{}",
      },
    );
  });

  it("rejects a response without a non-empty oidcToken", async () => {
    const fetchFn = vi.fn().mockResolvedValue({
      ok: true,
      status: 200,
      json: async () => ({ oidcToken: "" }),
    });

    await expect(requestOidcToken(material(), fetchFn)).rejects.toThrow();
  });
});

describe("refresh state machine", () => {
  it("publishes the initial assertion before readiness, then stops cleanly", async () => {
    let now = 1_700_000_000_000;
    const token = jwt(now / 1000 + 300);
    const controller = new AbortController();
    const { writes, files, writer } = recordingWriter();

    const rc = await runRefresher(
      material({ initialIdToken: token }),
      controller.signal,
      {
        now: () => now,
        writeAtomic: writer,
        sleep: async (ms) => {
          now += ms;
          controller.abort();
        },
        provider: { createOidcToken: vi.fn() },
      },
    );

    expect(rc).toBe(0);
    expect(writes[0]!.path).toBe("/state/status.json");
    expect(JSON.parse(writes[0]!.content)).toMatchObject({
      state: "starting",
    });
    expect(writes[1]).toEqual({
      path: "/state/token",
      content: token,
      mode: 0o644,
    });
    expect(writes[2]!.path).toBe("/state/status.json");
    expect(JSON.parse(writes[2]!.content)).toMatchObject({
      state: "ready",
    });
    expect(writes[3]!.path).toBe("/state/ready.json");
    expect(JSON.parse(writes[3]!.content)).toMatchObject({
      state: "ready",
    });
    expect(JSON.parse(files.get("/state/status.json")!.content)).toMatchObject({
      state: "stopped",
    });
  });

  it("requests and publishes a replacement at exp minus 60 seconds", async () => {
    let now = 1_700_000_000_000;
    const initial = jwt(now / 1000 + 300);
    const replacement = jwt(now / 1000 + 600);
    const controller = new AbortController();
    const { writes, writer } = recordingWriter();
    const provider = vi.fn().mockResolvedValue(replacement);
    let sleepCount = 0;

    const rc = await runRefresher(
      material({ initialIdToken: initial }),
      controller.signal,
      {
        now: () => now,
        writeAtomic: writer,
        provider: { createOidcToken: provider },
        sleep: async (ms) => {
          now += ms;
          sleepCount += 1;
          if (sleepCount === 2) controller.abort();
        },
      },
    );

    expect(rc).toBe(0);
    expect(provider).toHaveBeenCalledTimes(1);
    expect(provider.mock.invocationCallOrder[0]).toBeDefined();
    const tokenWrites = writes.filter((write) => write.path === "/state/token");
    expect(tokenWrites.map((write) => write.content)).toEqual([
      initial,
      replacement,
    ]);
    expect(
      statusDocuments(writes).some(
        (status) =>
          status.state === "refreshing" &&
          status.updatedAt === new Date(1_700_000_240_000).toISOString(),
      ),
    ).toBe(true);
  });

  it("uses the malformed-exp fallback and emits no token material in warnings", async () => {
    let now = 1_700_000_000_000;
    const controller = new AbortController();
    const { writer } = recordingWriter();
    const report = vi.fn();
    const provider = vi.fn().mockResolvedValue(jwt(now / 1000 + 600));
    let sleepCount = 0;

    const rc = await runRefresher(material(), controller.signal, {
      now: () => now,
      writeAtomic: writer,
      report,
      provider: { createOidcToken: provider },
      sleep: async (ms) => {
        if (sleepCount === 0) expect(ms).toBe(240_000);
        now += ms;
        sleepCount += 1;
        if (sleepCount === 2) controller.abort();
      },
    });

    expect(rc).toBe(0);
    expect(provider).toHaveBeenCalledTimes(1);
    const output = report.mock.calls.flat().join("\n");
    expect(output).toContain("conservative timing");
    expect(output).not.toContain(INITIAL_TOKEN);
    expect(output).not.toContain(SYSTEM_TOKEN);
  });

  it("retries transient failures with capped exponential backoff", async () => {
    let now = 1_700_000_000_000;
    const initial = jwt(now / 1000 + 180);
    const replacement = jwt(now / 1000 + 600);
    const controller = new AbortController();
    const { writes, writer } = recordingWriter();
    const error = Object.assign(new Error("throttled"), { statusCode: 429 });
    const provider = vi
      .fn()
      .mockRejectedValueOnce(error)
      .mockResolvedValueOnce(replacement);
    const sleeps: number[] = [];

    const rc = await runRefresher(
      material({ initialIdToken: initial }),
      controller.signal,
      {
        now: () => now,
        writeAtomic: writer,
        provider: { createOidcToken: provider },
        sleep: async (ms) => {
          sleeps.push(ms);
          now += ms;
          if (provider.mock.calls.length === 2) controller.abort();
        },
      },
    );

    expect(rc).toBe(0);
    expect(provider).toHaveBeenCalledTimes(2);
    expect(sleeps.slice(0, 2)).toEqual([120_000, 1_000]);
    expect(statusDocuments(writes)).toContainEqual(
      expect.objectContaining({
        state: "refreshing",
        errorCategory: "throttled",
      }),
    );
  });

  it("rejects an empty refresh without overwriting the current assertion", async () => {
    let now = 1_700_000_000_000;
    const initial = jwt(now / 1000 + 61);
    const { writes, writer } = recordingWriter();

    const rc = await runRefresher(
      material({ initialIdToken: initial }),
      new AbortController().signal,
      {
        now: () => now,
        writeAtomic: writer,
        provider: { createOidcToken: vi.fn().mockResolvedValue("") },
        sleep: async (ms) => {
          now += ms;
        },
      },
    );

    expect(rc).toBe(1);
    expect(
      writes.filter((write) => write.path === "/state/token"),
    ).toHaveLength(1);
    expect(statusDocuments(writes).at(-1)).toMatchObject({
      state: "unhealthy",
      errorCategory: "invalid-response",
    });
  });

  it("becomes unhealthy only after refresh failures outlive the assertion", async () => {
    let now = 1_700_000_000_000;
    const initial = jwt(now / 1000 + 62);
    const { writes, writer } = recordingWriter();
    const provider = vi
      .fn()
      .mockRejectedValue(Object.assign(new Error("server body"), {
        statusCode: 503,
      }));

    const rc = await runRefresher(
      material({ initialIdToken: initial }),
      new AbortController().signal,
      {
        now: () => now,
        writeAtomic: writer,
        provider: { createOidcToken: provider },
        sleep: async (ms) => {
          now += ms;
        },
      },
    );

    expect(rc).toBe(1);
    expect(provider.mock.calls.length).toBeGreaterThan(1);
    expect(statusDocuments(writes).at(-1)).toMatchObject({
      state: "unhealthy",
      errorCategory: "server",
    });
  });

  it("redacts both credentials from diagnostics and persisted status", async () => {
    let now = 1_700_000_000_000;
    const initial = jwt(now / 1000 + 61);
    const { writes, writer } = recordingWriter();
    const report = vi.fn();
    const credentialError = new Error(
      `request failed with ${INITIAL_TOKEN} and ${SYSTEM_TOKEN}`,
    );

    const rc = await runRefresher(
      material({ initialIdToken: initial }),
      new AbortController().signal,
      {
        now: () => now,
        writeAtomic: writer,
        report,
        provider: {
          createOidcToken: vi.fn().mockRejectedValue(credentialError),
        },
        sleep: async (ms) => {
          now += ms;
        },
      },
    );

    expect(rc).toBe(1);
    const observable = [
      ...report.mock.calls.flat().map(String),
      ...writes
        .filter((write) => write.path !== "/state/token")
        .map((write) => write.content),
    ].join("\n");
    expect(observable).not.toContain(INITIAL_TOKEN);
    expect(observable).not.toContain(SYSTEM_TOKEN);
  });
});
