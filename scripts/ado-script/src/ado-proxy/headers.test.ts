import { describe, expect, it } from "vitest";

import {
  sanitizePackageRequestHeaders,
  sanitizePackageResponseHeaders,
  sanitizeRequestHeaders,
  sanitizeResponseHeaders,
} from "./headers.js";

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

describe("sanitizePackageRequestHeaders", () => {
  it("strips client credentials, including package-client ones", () => {
    const { headers, strippedCredentials } = sanitizePackageRequestHeaders(
      {
        authorization: "Bearer sentinel",
        "proxy-authorization": "Basic abc",
        cookie: "a=b",
        "x-nuget-apikey": "push-key",
        "npm-otp": "123456",
      },
      "pkgs.dev.azure.com",
    );
    expect(headers.authorization).toBeUndefined();
    expect(headers["x-nuget-apikey"]).toBeUndefined();
    expect(headers["npm-otp"]).toBeUndefined();
    expect(headers.cookie).toBeUndefined();
    expect(strippedCredentials).toEqual(
      expect.arrayContaining(["authorization", "proxy-authorization", "cookie", "x-nuget-apikey", "npm-otp"]),
    );
  });

  it("keeps the conditional, ranged, and protocol headers package clients send", () => {
    const { headers } = sanitizePackageRequestHeaders(
      {
        Accept: "application/vnd.npm.install-v1+json",
        "If-None-Match": '"etag"',
        "if-modified-since": "Mon, 01 Jan 2024 00:00:00 GMT",
        range: "bytes=0-99",
        "if-range": '"etag"',
        "npm-command": "install",
        "npm-session": "abc",
        "x-nuget-session-id": "s",
        "x-nuget-protocol-version": "4.1.0",
        "user-agent": "npm/10",
      },
      "pkgs.dev.azure.com",
    );
    expect(headers).toMatchObject({
      accept: "application/vnd.npm.install-v1+json",
      "if-none-match": '"etag"',
      range: "bytes=0-99",
      "if-range": '"etag"',
      "npm-command": "install",
      "x-nuget-session-id": "s",
      "user-agent": "npm/10",
    });
  });

  it("drops forwarding, smuggling, and unknown headers, and pins the protocol ones", () => {
    const { headers } = sanitizePackageRequestHeaders(
      {
        host: "evil.test",
        forwarded: "for=1.2.3.4",
        "x-forwarded-for": "1.2.3.4",
        "x-forwarded-host": "evil.test",
        "transfer-encoding": "chunked",
        "x-http-method-override": "PUT",
        referer: "install secret-thing",
        "accept-encoding": "gzip",
        connection: "keep-alive, x-secret",
        "x-secret": "1",
      },
      "pkgs.dev.azure.com",
    );
    expect(headers).toEqual({
      host: "pkgs.dev.azure.com",
      "x-tfs-fedauthredirect": "Suppress",
      "accept-encoding": "identity",
      connection: "close",
    });
  });
});

describe("sanitizePackageResponseHeaders", () => {
  it("relays caching and download headers but no session or auth material", () => {
    const headers = sanitizePackageResponseHeaders({
      "content-type": "application/octet-stream",
      "content-length": "10",
      etag: '"e"',
      "last-modified": "x",
      "content-disposition": "attachment; filename=x.tgz",
      "set-cookie": ["a=b"],
      "www-authenticate": "Bearer",
      location: "https://evil.test/",
      "x-vss-userdata": "id:name",
      "strict-transport-security": "max-age=1",
    });
    expect(headers).toEqual({
      "content-type": "application/octet-stream",
      "content-length": "10",
      etag: '"e"',
      "last-modified": "x",
      "content-disposition": "attachment; filename=x.tgz",
    });
  });
});
