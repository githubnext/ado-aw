import { describe, it, expect, vi } from "vitest";
import { generateKeyPairSync, createVerify } from "node:crypto";

import {
  buildAppJwt,
  parseArgs,
  parseRepositories,
  resolveInstallationId,
  mintInstallationToken,
  revoke,
  main,
} from "../index.js";

function makeKeyPair() {
  return generateKeyPairSync("rsa", {
    modulusLength: 2048,
    publicKeyEncoding: { type: "spki", format: "pem" },
    privateKeyEncoding: { type: "pkcs8", format: "pem" },
  });
}

function decodeSegment(seg: string): unknown {
  const b64 = seg.replace(/-/g, "+").replace(/_/g, "/");
  return JSON.parse(Buffer.from(b64, "base64").toString("utf8"));
}

/** Build a minimal fetch-like Response stub. */
function jsonResponse(status: number, body: unknown) {
  return {
    ok: status >= 200 && status < 300,
    status,
    json: async () => body,
    text: async () => (typeof body === "string" ? body : JSON.stringify(body)),
  };
}

describe("buildAppJwt", () => {
  it("produces a verifiable RS256 JWT with iat/exp/iss claims", () => {
    const { publicKey, privateKey } = makeKeyPair();
    const now = 1_700_000_000;
    const jwt = buildAppJwt("123456", privateKey, now);
    const [h, p, s] = jwt.split(".") as [string, string, string];
    expect(h).toBeTruthy();
    expect(p).toBeTruthy();
    expect(s).toBeTruthy();

    const header = decodeSegment(h) as { alg: string; typ: string };
    expect(header).toEqual({ alg: "RS256", typ: "JWT" });

    const payload = decodeSegment(p) as {
      iat: number;
      exp: number;
      iss: string;
    };
    expect(payload.iss).toBe("123456");
    expect(payload.iat).toBe(now - 60);
    expect(payload.exp).toBe(now + 540);

    const verifier = createVerify("RSA-SHA256");
    verifier.update(`${h}.${p}`);
    verifier.end();
    const sig = Buffer.from(
      s.replace(/-/g, "+").replace(/_/g, "/"),
      "base64",
    );
    expect(verifier.verify(publicKey, sig)).toBe(true);
  });
});

describe("parseRepositories", () => {
  it("splits on commas, spaces, and newlines and drops blanks", () => {
    expect(parseRepositories("a, b\nc  d")).toEqual(["a", "b", "c", "d"]);
    expect(parseRepositories(undefined)).toEqual([]);
    expect(parseRepositories("")).toEqual([]);
    expect(parseRepositories("  single  ")).toEqual(["single"]);
  });
});

describe("resolveInstallationId", () => {
  it("returns the id from the org endpoint when it succeeds", async () => {
    const fetchFn = vi.fn().mockResolvedValueOnce(jsonResponse(200, { id: 42 }));
    const id = await resolveInstallationId(
      fetchFn as never,
      "https://api.github.com",
      "jwt",
      "octo-org",
    );
    expect(id).toBe(42);
    expect(fetchFn).toHaveBeenCalledTimes(1);
    expect(fetchFn.mock.calls[0]![0]).toContain("/orgs/octo-org/installation");
  });

  it("falls back to the user endpoint when the org lookup 404s", async () => {
    const fetchFn = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse(404, "not found"))
      .mockResolvedValueOnce(jsonResponse(200, { id: 7 }));
    const id = await resolveInstallationId(
      fetchFn as never,
      "https://api.github.com",
      "jwt",
      "octo-user",
    );
    expect(id).toBe(7);
    expect(fetchFn).toHaveBeenCalledTimes(2);
    expect(fetchFn.mock.calls[1]![0]).toContain("/users/octo-user/installation");
  });

  it("throws when neither endpoint resolves", async () => {
    const fetchFn = vi
      .fn()
      .mockResolvedValue(jsonResponse(404, "nope"));
    await expect(
      resolveInstallationId(
        fetchFn as never,
        "https://api.github.com",
        "jwt",
        "ghost",
      ),
    ).rejects.toThrow(/could not resolve/i);
  });
});

describe("mintInstallationToken", () => {
  it("scopes to repositories when provided and returns the token", async () => {
    const fetchFn = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse(201, { token: "ghs_secret" }));
    const token = await mintInstallationToken(
      fetchFn as never,
      "https://api.github.com",
      "jwt",
      99,
      ["repo-a", "repo-b"],
    );
    expect(token).toBe("ghs_secret");
    const [url, init] = fetchFn.mock.calls[0]!;
    expect(url).toContain("/app/installations/99/access_tokens");
    expect(init.method).toBe("POST");
    expect(JSON.parse(init.body)).toEqual({
      repositories: ["repo-a", "repo-b"],
    });
  });

  it("omits the repositories field when none are given", async () => {
    const fetchFn = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse(201, { token: "ghs_all" }));
    await mintInstallationToken(
      fetchFn as never,
      "https://api.github.com",
      "jwt",
      1,
      [],
    );
    expect(JSON.parse(fetchFn.mock.calls[0]![1].body)).toEqual({});
  });

  it("throws on a non-2xx response", async () => {
    const fetchFn = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse(403, "forbidden"));
    await expect(
      mintInstallationToken(
        fetchFn as never,
        "https://api.github.com",
        "jwt",
        1,
        [],
      ),
    ).rejects.toThrow(/HTTP 403/);
  });
});

describe("parseArgs", () => {
  it("parses the mint flags into CliArgs", () => {
    const args = parseArgs([
      "--app-id",
      "1234567",
      "--owner",
      "octo-org",
      "--output-var",
      "GITHUB_APP_TOKEN",
      "--repositories",
      "repo-a repo-b",
      "--api-url",
      "https://ghe.example.com/api/v3",
    ]);
    expect(args).toEqual({
      appId: "1234567",
      owner: "octo-org",
      outputVar: "GITHUB_APP_TOKEN",
      repositories: "repo-a repo-b",
      apiUrl: "https://ghe.example.com/api/v3",
    });
  });

  it("ignores unknown flags and tolerates a bare revoke arg list", () => {
    expect(parseArgs(["--api-url", "https://x/api/v3"])).toEqual({
      apiUrl: "https://x/api/v3",
    });
    expect(parseArgs(["--unknown", "v"])).toEqual({});
    expect(parseArgs([])).toEqual({});
  });

  it("stays aligned when a value-less boolean flag precedes real flags", () => {
    // A future compiler adds `--debug` (no value); an older bundle must not
    // swallow the following `--app-id` as --debug's value.
    const args = parseArgs([
      "--debug",
      "--app-id",
      "1234567",
      "--owner",
      "octo-org",
    ]);
    expect(args.appId).toBe("1234567");
    expect(args.owner).toBe("octo-org");
  });

  it("does not consume a following flag as a value", () => {
    // `--repositories` with no value (immediately followed by another flag)
    // leaves repositories unset rather than eating `--owner`.
    const args = parseArgs(["--repositories", "--owner", "octo-org"]);
    expect(args.repositories).toBeUndefined();
    expect(args.owner).toBe("octo-org");
  });
});

describe("main", () => {
  it("mints a token and emits a masked same-job variable", async () => {
    const { privateKey } = makeKeyPair();
    const fetchFn = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse(200, { id: 55 }))
      .mockResolvedValueOnce(jsonResponse(201, { token: "ghs_minted" }));
    const writes: string[] = [];
    const spy = vi
      .spyOn(process.stdout, "write")
      .mockImplementation((chunk: string | Uint8Array): boolean => {
        writes.push(chunk.toString());
        return true;
      });

    const rc = await main(
      {
        appId: "123",
        owner: "octo-org",
        outputVar: "GITHUB_APP_TOKEN",
        repositories: "repo-a repo-b",
      },
      { GH_APP_PRIVATE_KEY: privateKey } as NodeJS.ProcessEnv,
      fetchFn as never,
    );
    spy.mockRestore();

    expect(rc).toBe(0);
    const out = writes.join("");
    expect(out).toContain(
      "##vso[task.setvariable variable=GITHUB_APP_TOKEN;issecret=true]ghs_minted",
    );
  });

  it("honours --api-url and --output-var", async () => {
    const { privateKey } = makeKeyPair();
    const fetchFn = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse(200, { id: 1 }))
      .mockResolvedValueOnce(jsonResponse(201, { token: "ghs_ghes" }));
    const writes: string[] = [];
    const spy = vi
      .spyOn(process.stdout, "write")
      .mockImplementation((chunk: string | Uint8Array): boolean => {
        writes.push(chunk.toString());
        return true;
      });

    const rc = await main(
      {
        appId: "1",
        owner: "acme",
        apiUrl: "https://ghes.example.com/api/v3/",
        outputVar: "CUSTOM_TOKEN",
      },
      { GH_APP_PRIVATE_KEY: privateKey } as NodeJS.ProcessEnv,
      fetchFn as never,
    );
    spy.mockRestore();

    expect(rc).toBe(0);
    // Trailing slash stripped; org endpoint queried on the GHES host.
    expect(fetchFn.mock.calls[0]![0]).toBe(
      "https://ghes.example.com/api/v3/orgs/acme/installation",
    );
    expect(writes.join("")).toContain(
      "##vso[task.setvariable variable=CUSTOM_TOKEN;issecret=true]ghs_ghes",
    );
  });

  it("returns 1 and logs an error when a required arg is missing", async () => {
    const writes: string[] = [];
    const spy = vi
      .spyOn(process.stdout, "write")
      .mockImplementation((chunk: string | Uint8Array): boolean => {
        writes.push(chunk.toString());
        return true;
      });
    // --owner missing.
    const rc = await main(
      { appId: "1" },
      { GH_APP_PRIVATE_KEY: "key" } as NodeJS.ProcessEnv,
      vi.fn() as never,
    );
    spy.mockRestore();
    expect(rc).toBe(1);
    expect(writes.join("")).toContain("--owner");
  });

  it("returns 1 when the private key env secret is missing", async () => {
    const writes: string[] = [];
    const spy = vi
      .spyOn(process.stdout, "write")
      .mockImplementation((chunk: string | Uint8Array): boolean => {
        writes.push(chunk.toString());
        return true;
      });
    const rc = await main(
      { appId: "1", owner: "acme" },
      {} as NodeJS.ProcessEnv,
      vi.fn() as never,
    );
    spy.mockRestore();
    expect(rc).toBe(1);
    expect(writes.join("")).toContain("GH_APP_PRIVATE_KEY");
  });
});

describe("revoke", () => {
  it("DELETEs the installation token and returns 0", async () => {
    const fetchFn = vi.fn().mockResolvedValueOnce(jsonResponse(204, ""));
    const rc = await revoke(
      { apiUrl: "https://ghe.example.com/api/v3/" },
      { GH_APP_TOKEN: "ghs_minted" } as NodeJS.ProcessEnv,
      fetchFn as never,
    );
    expect(rc).toBe(0);
    const [url, init] = fetchFn.mock.calls[0]!;
    expect(url).toBe("https://ghe.example.com/api/v3/installation/token");
    expect(init.method).toBe("DELETE");
    expect(init.headers.Authorization).toBe("Bearer ghs_minted");
  });

  it("is a no-op (returns 0) when no token is present", async () => {
    const fetchFn = vi.fn();
    const rc = await revoke({}, {} as NodeJS.ProcessEnv, fetchFn as never);
    expect(rc).toBe(0);
    expect(fetchFn).not.toHaveBeenCalled();
  });

  it("never fails the build when the DELETE errors", async () => {
    const fetchFn = vi.fn().mockRejectedValueOnce(new Error("network down"));
    const rc = await revoke(
      {},
      { GH_APP_TOKEN: "ghs_minted" } as NodeJS.ProcessEnv,
      fetchFn as never,
    );
    expect(rc).toBe(0);
  });

  it("tolerates a non-2xx DELETE response", async () => {
    const fetchFn = vi.fn().mockResolvedValueOnce(jsonResponse(404, "gone"));
    const rc = await revoke(
      {},
      { GH_APP_TOKEN: "ghs_minted" } as NodeJS.ProcessEnv,
      fetchFn as never,
    );
    expect(rc).toBe(0);
  });
});
