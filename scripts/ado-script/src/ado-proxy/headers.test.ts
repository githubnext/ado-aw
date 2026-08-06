import { describe, expect, it } from "vitest";

import { sanitizeRequestHeaders, sanitizeResponseHeaders } from "./headers.js";

describe("sanitizeRequestHeaders", () => {
  it("strips every client-supplied credential", () => {
    const { headers, strippedCredentials } = sanitizeRequestHeaders(
      {
        authorization: "Basic OnNlbnRpbmVs",
        "proxy-authorization": "Basic abc",
        cookie: "UserAuthentication=x",
      },
      "dev.azure.com",
    );
    // The injected bearer is applied by the caller *after* the allow decision;
    // nothing the client sent may influence the upstream identity.
    expect(headers.authorization).toBeUndefined();
    expect(headers.cookie).toBeUndefined();
    expect(strippedCredentials).toEqual(
      expect.arrayContaining(["authorization", "proxy-authorization", "cookie"]),
    );
  });

  it("drops headers that could change what the upstream believes the request is", () => {
    const { headers } = sanitizeRequestHeaders(
      {
        "x-http-method-override": "POST",
        "x-original-url": "/other/_apis/serviceendpoint",
        "x-forwarded-host": "evil.test",
        "transfer-encoding": "chunked",
        forwarded: "for=1.2.3.4",
      },
      "dev.azure.com",
    );
    expect(Object.keys(headers).sort()).toEqual([
      "accept-encoding",
      "connection",
      "host",
      "x-tfs-fedauthredirect",
    ]);
  });

  it("forwards the negotiation and correlation headers Azure DevOps needs", () => {
    const { headers } = sanitizeRequestHeaders(
      {
        accept: "application/json;api-version=7.1",
        "user-agent": "azure-devops-cli",
        "x-ms-continuationtoken": "abc",
        "content-type": "application/json",
      },
      "dev.azure.com",
    );
    expect(headers.accept).toBe("application/json;api-version=7.1");
    expect(headers["user-agent"]).toBe("azure-devops-cli");
    expect(headers["x-ms-continuationtoken"]).toBe("abc");
  });

  it("always suppresses the federated-auth redirect", () => {
    // Without this Azure DevOps answers an auth failure with a 203 sign-in
    // page, which clients surface as unparseable HTML rather than a 401.
    const { headers } = sanitizeRequestHeaders(
      { "x-tfs-fedauthredirect": "Auto" },
      "dev.azure.com",
    );
    expect(headers["x-tfs-fedauthredirect"]).toBe("Suppress");
  });

  it("pins the Host header to the intercepted host", () => {
    const { headers } = sanitizeRequestHeaders({ host: "evil.test" }, "dev.azure.com");
    expect(headers.host).toBe("dev.azure.com");
  });

  it("requests identity encoding", () => {
    // Response filtering and the byte budget both operate on the plain body.
    const { headers } = sanitizeRequestHeaders({ "accept-encoding": "gzip" }, "dev.azure.com");
    expect(headers["accept-encoding"]).toBe("identity");
  });

  it("takes only the first value of a repeated header", () => {
    const { headers } = sanitizeRequestHeaders(
      { accept: ["application/json;api-version=7.1", "application/json;api-version=1.0"] },
      "dev.azure.com",
    );
    expect(headers.accept).toBe("application/json;api-version=7.1");
  });
});

describe("sanitizeResponseHeaders", () => {
  it("keeps only the safe response headers", () => {
    const headers = sanitizeResponseHeaders({
      "content-type": "application/json",
      "x-ms-continuationtoken": "next",
      "set-cookie": ["UserAuthentication=x"],
      "www-authenticate": "Bearer realm=...",
      location: "https://artifacts.example/signed?sig=abc",
    });
    // `set-cookie` and `www-authenticate` would hand the agent session material
    // or provoke an interactive login; `location` is how a signed URL escapes.
    expect(headers).toEqual({
      "content-type": "application/json",
      "x-ms-continuationtoken": "next",
    });
  });
});
