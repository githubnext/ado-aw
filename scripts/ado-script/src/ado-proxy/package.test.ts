/**
 * Unit tests for package-feed authorization and redirect validation.
 *
 * The package family authorizes by URL *shape* rather than a closed catalog,
 * so most of the attack surface is in parsing: an encoded separator, a
 * traversal, or a doubly encoded segment that the policy reads one way and
 * Azure Artifacts another. Each such form is pinned here as a stable denial.
 */
import { describe, expect, it } from "vitest";

import { CATALOG_SCHEMA_VERSION, PACKAGE_PROTOCOLS } from "./catalog.js";
import type { ProxyPolicy } from "./config.js";
import {
  authorizePackageRedirect,
  authorizePackageRequest,
  isAllowedRedirectHost,
  isPackageHost,
  packageAuthorizationHeader,
  type PackageDecision,
} from "./package.js";

const PROJECT_ID = "11111111-1111-1111-1111-111111111111";
const FEED_ID = "22222222-2222-2222-2222-222222222222";
const VIEW_ID = "33333333-3333-3333-3333-333333333333";
const HOST = "pkgs.dev.azure.com";

const POLICY: ProxyPolicy = {
  catalog_version: CATALOG_SCHEMA_VERSION,
  organization: "contoso",
  project: "Engineering",
  capabilities: [],
  protected_hosts: ["dev.azure.com", "app.vssps.visualstudio.com"],
  allowed_resource_areas: [],
  packages: {
    hosts: [HOST],
    feeds: [
      {
        organization: "contoso",
        project: "Engineering",
        feed: "internal",
        view: "Release",
        protocols: ["npm", "nuget"],
        project_id: PROJECT_ID,
        feed_id: FEED_ID,
        view_id: VIEW_ID,
      },
      {
        organization: "contoso",
        feed: "shared",
        protocols: ["pypi", "cargo"],
      },
    ],
  },
};

const INTERNAL = "/contoso/Engineering/_packaging/internal@Release";
const SHARED = "/contoso/_packaging/shared";

function check(rawTarget: string, method = "GET", policy: ProxyPolicy = POLICY): PackageDecision {
  return authorizePackageRequest({ method, host: HOST, rawTarget }, policy);
}

function expectAllow(rawTarget: string, protocol: string, method = "GET"): void {
  const decision = check(rawTarget, method);
  expect(decision, rawTarget).toMatchObject({ allow: true });
  if (decision.allow) expect(decision.protocol.protocol).toBe(protocol);
}

function expectDeny(rawTarget: string, reason: string, method = "GET"): void {
  expect(check(rawTarget, method), rawTarget).toMatchObject({ allow: false, reason });
}

describe("isPackageHost", () => {
  it("matches only when the policy carries a packages section", () => {
    expect(isPackageHost(HOST, POLICY)).toBe(true);
    expect(isPackageHost("PKGS.DEV.AZURE.COM.", POLICY)).toBe(true);
    expect(isPackageHost(`${HOST}:443`, POLICY)).toBe(true);
    const { packages: _packages, ...withoutPackages } = POLICY;
    expect(isPackageHost(HOST, withoutPackages)).toBe(false);
  });

  it("never matches a look-alike", () => {
    expect(isPackageHost("pkgs.dev.azure.com.evil.test", POLICY)).toBe(false);
    expect(isPackageHost("evilpkgs.dev.azure.com", POLICY)).toBe(false);
    expect(isPackageHost("dev.azure.com", POLICY)).toBe(false);
    expect(isPackageHost("", POLICY)).toBe(false);
  });
});

describe("authorizePackageRequest — allowed shapes", () => {
  it("allows a project-scoped npm read through the granted view", () => {
    const decision = check(`${INTERNAL}/npm/registry/lodash`);
    expect(decision.allow).toBe(true);
    if (!decision.allow) return;
    expect(decision.protocol.protocol).toBe("npm");
    expect(decision.feed.feed).toBe("internal");
  });

  it("allows HEAD as well as GET", () => {
    expectAllow(`${INTERNAL}/npm/registry/lodash`, "npm", "HEAD");
  });

  it("matches organization, project, feed, view, and prefix case-insensitively", () => {
    expectAllow("/CONTOSO/engineering/_PACKAGING/INTERNAL@release/NPM/Registry/lodash", "npm");
  });

  it("matches the GUID forms of project, feed, and view", () => {
    expectAllow(`/contoso/${PROJECT_ID}/_packaging/${FEED_ID}@${VIEW_ID}/nuget/v3/index.json`, "nuget");
    expectAllow(
      `/contoso/${PROJECT_ID.toUpperCase()}/_packaging/internal@${VIEW_ID}/nuget/v3/index.json`,
      "nuget",
    );
  });

  it("accepts the view separator percent-encoded", () => {
    expectAllow("/contoso/Engineering/_packaging/internal%40Release/npm/registry/lodash", "npm");
  });

  it("allows an organization-scoped feed with any view, or none", () => {
    expectAllow(`${SHARED}/pypi/simple/requests/`, "pypi");
    expectAllow(`${SHARED}/pypi/download/requests/2.0/requests-2.0-py3-none-any.whl`, "pypi");
    expectAllow(`${SHARED}@Release/cargo/index/config.json`, "cargo");
    expectAllow(`${SHARED}@Local/cargo/index/se/rd/serde`, "cargo");
  });

  it("allows the registry root with its trailing slash", () => {
    expectAllow(`${INTERNAL}/npm/registry/`, "npm");
    expectAllow(`${SHARED}/pypi/simple/`, "pypi");
  });

  it("allows an npm scoped package name with an encoded slash", () => {
    expectAllow(`${INTERNAL}/npm/registry/@types%2fnode`, "npm");
    expectAllow(`${INTERNAL}/npm/registry/@types%2Fnode`, "npm");
    expectAllow(`${INTERNAL}/npm/registry/@types/node/-/node-20.0.0.tgz`, "npm");
  });

  it("passes the query through untouched", () => {
    expectAllow(`${INTERNAL}/nuget/v3/query2/?q=Newtonsoft&take=20&prerelease=true`, "nuget");
    expectAllow(`${INTERNAL}/npm/registry/lodash?write=true&x=%0d%0a`, "npm");
  });
});

describe("authorizePackageRequest — scope denials", () => {
  it("denies a project-scoped grant addressed without its project", () => {
    expectDeny("/contoso/_packaging/internal@Release/npm/registry/lodash", "feed-not-granted");
  });

  it("denies an organization-scoped grant addressed through a project", () => {
    expectDeny("/contoso/Engineering/_packaging/shared/pypi/simple/requests/", "feed-not-granted");
  });

  it("denies another organization, project, or feed", () => {
    expectDeny("/fabrikam/Engineering/_packaging/internal@Release/npm/registry/x", "feed-not-granted");
    expectDeny("/contoso/Secrets/_packaging/internal@Release/npm/registry/x", "feed-not-granted");
    expectDeny("/contoso/Engineering/_packaging/private@Release/npm/registry/x", "feed-not-granted");
    expectDeny(`/contoso/${FEED_ID}/_packaging/internal@Release/npm/registry/x`, "feed-not-granted");
  });

  it("denies a pinned feed addressed without a view or through another view", () => {
    // The bare feed includes unpromoted packages the author did not expose.
    expectDeny("/contoso/Engineering/_packaging/internal/npm/registry/lodash", "view-not-granted");
    expectDeny("/contoso/Engineering/_packaging/internal@Local/npm/registry/lodash", "view-not-granted");
    expectDeny(
      `/contoso/Engineering/_packaging/internal@${PROJECT_ID}/npm/registry/lodash`,
      "view-not-granted",
    );
  });

  it("denies a protocol the grant does not enable", () => {
    expectDeny(`${INTERNAL}/pypi/simple/requests/`, "protocol-not-granted");
    expectDeny(`${SHARED}/npm/registry/lodash`, "protocol-not-granted");
  });

  it("denies everything when the host is not a policed package host", () => {
    expect(
      authorizePackageRequest(
        { method: "GET", host: "dev.azure.com", rawTarget: `${INTERNAL}/npm/registry/x` },
        POLICY,
      ),
    ).toMatchObject({ allow: false, reason: "unknown-host" });
    const { packages: _packages, ...withoutPackages } = POLICY;
    expect(check(`${INTERNAL}/npm/registry/x`, "GET", withoutPackages)).toMatchObject({
      allow: false,
      reason: "unknown-host",
    });
  });
});

describe("authorizePackageRequest — route denials", () => {
  it("denies the packaging management REST surface", () => {
    expectDeny("/contoso/_apis/packaging/feeds", "unknown-route");
    expectDeny("/contoso/_apis/packaging/feeds/internal/packages", "unknown-route");
    expectDeny("/contoso/Engineering/_apis/packaging/feeds/internal", "unknown-route");
    expectDeny("/contoso/_apis/connectionData", "unknown-route");
  });

  it("denies paths with no feed or nothing under it", () => {
    expectDeny("/", "unknown-route");
    expectDeny("/contoso", "unknown-route");
    expectDeny("/contoso/_packaging", "unknown-route");
    expectDeny("/contoso/_packaging/shared", "unknown-route");
    expectDeny("/contoso/_packaging/shared/", "unknown-route");
    expectDeny("/_packaging/shared/pypi/simple/x/", "unknown-route");
  });

  it("denies a path under the feed that no protocol catalogues", () => {
    expectDeny(`${SHARED}/maven/v1/org/x.jar`, "unknown-route");
    expectDeny(`${INTERNAL}/nuget/v2/Packages`, "unknown-route");
    // A prefix ends in '/', so the bare prefix is not itself a route.
    expectDeny(`${INTERNAL}/npm/registry`, "unknown-route");
    // One encoded segment cannot assemble a two-segment prefix.
    expectDeny(`${INTERNAL}/npm%2fregistry/lodash`, "unknown-route");
  });

  it("denies a malformed feed@view segment", () => {
    expectDeny("/contoso/_packaging/shared@/pypi/simple/x/", "malformed-target");
    expectDeny("/contoso/_packaging/@Release/pypi/simple/x/", "malformed-target");
    expectDeny("/contoso/_packaging/shared@a@b/pypi/simple/x/", "malformed-target");
  });
});

describe("authorizePackageRequest — method denials", () => {
  it.each([
    ["POST", `${INTERNAL}/npm/registry/-/npm/v1/security/audits`],
    ["POST", `${INTERNAL}/npm/registry/-/npm/v1/security/advisories/bulk`],
    ["PUT", `${INTERNAL}/npm/registry/my-package`],
    ["DELETE", `${INTERNAL}/npm/registry/my-package/-rev/1`],
    ["PATCH", `${INTERNAL}/npm/registry/my-package`],
    ["PUT", `${INTERNAL}/nuget/v3/index.json`],
    ["POST", `${SHARED}/pypi/upload/`],
    ["OPTIONS", `${INTERNAL}/npm/registry/lodash`],
    ["get", `${INTERNAL}/npm/registry/lodash`],
  ])("denies %s", (method, target) => {
    expectDeny(target, "method-not-read", method);
  });

  it("denies a write even to a path no protocol catalogues", () => {
    expectDeny("/contoso/_apis/packaging/feeds", "method-not-read", "POST");
  });
});

describe("authorizePackageRequest — encoding denials", () => {
  it("denies an encoded slash outside npm", () => {
    expectDeny(`${SHARED}/pypi/simple/a%2fb/`, "encoded-separator");
    expectDeny(`${SHARED}/cargo/index/a%2Fb`, "encoded-separator");
  });

  it("denies an encoded slash in npm that is not a scoped name", () => {
    expectDeny(`${INTERNAL}/npm/registry/foo%2fbar`, "encoded-separator");
    expectDeny(`${INTERNAL}/npm/registry/@scope%2fa%2fb`, "encoded-separator");
    expectDeny(`${INTERNAL}/npm/registry/@%2fname`, "encoded-separator");
  });

  it("denies an encoded slash in the organization, project, or feed segment", () => {
    expectDeny("/con%2ftoso/Engineering/_packaging/internal@Release/npm/registry/x", "encoded-separator");
    expectDeny("/contoso/Engi%2fneering/_packaging/internal@Release/npm/registry/x", "encoded-separator");
    expectDeny("/contoso/Engineering/_packaging/inter%2Fnal@Release/npm/registry/x", "encoded-separator");
    expectDeny("/contoso/Engineering/_packaging/internal@Rel%2fease/npm/registry/x", "encoded-separator");
  });

  it("denies traversal in every spelling", () => {
    expectDeny(`${SHARED}/pypi/simple/../../_apis/packaging/feeds`, "path-traversal");
    expectDeny(`${SHARED}/pypi/simple/./requests/`, "path-traversal");
    expectDeny(`${SHARED}/pypi/simple/%2e%2e/x/`, "path-traversal");
    expectDeny(`${SHARED}/pypi/simple/%2E./x/`, "path-traversal");
  });

  it("denies double encoding", () => {
    expectDeny(`${INTERNAL}/npm/registry/@types%252fnode`, "double-encoding");
    expectDeny(`${SHARED}/pypi/simple/%252e%252e/`, "double-encoding");
    expectDeny(`${SHARED}/pypi/simple/100%25/`, "double-encoding");
  });

  it("denies invalid percent escapes", () => {
    expectDeny(`${SHARED}/pypi/simple/a%zz/`, "malformed-target");
    expectDeny(`${SHARED}/pypi/simple/a%/`, "malformed-target");
    expectDeny(`${SHARED}/pypi/simple/a%2/`, "malformed-target");
    expectDeny(`${SHARED}/pypi/simple/%c3%28/`, "malformed-target");
  });

  it("denies backslashes, raw or encoded", () => {
    expectDeny(`${SHARED}/pypi/simple\\..\\x/`, "malformed-target");
    expectDeny(`${SHARED}/pypi/simple/a%5cb/`, "malformed-target");
  });

  it("denies empty segments", () => {
    expectDeny(`${SHARED}/pypi//simple/x/`, "malformed-target");
    expectDeny(`/contoso//_packaging/shared/pypi/simple/x/`, "malformed-target");
    expectDeny(`//contoso/_packaging/shared/pypi/simple/x/`, "malformed-target");
  });

  it("denies control characters, spaces, NUL, and non-ASCII", () => {
    expectDeny(`${SHARED}/pypi/simple/a%00b/`, "malformed-target");
    expectDeny(`${SHARED}/pypi/simple/a%0d%0ab/`, "malformed-target");
    expectDeny(`${SHARED}/pypi/simple/a\r\nb/`, "malformed-target");
    expectDeny(`${SHARED}/pypi/simple/a b/`, "malformed-target");
    expectDeny(`${SHARED}/pypi/simple/é/`, "malformed-target");
    expectDeny(`${SHARED}/pypi/simple/x/?q=a\u0000b`, "malformed-target");
  });

  it("denies absolute-form targets and fragments", () => {
    expectDeny(`https://${HOST}${SHARED}/pypi/simple/x/`, "malformed-target");
    expectDeny(`${SHARED}/pypi/simple/x/#frag`, "malformed-target");
    expectDeny("*", "malformed-target");
  });

  it("never echoes request content into the denial detail", () => {
    const marker = "secretmarker7f3a";
    for (const target of [
      `/contoso/Engineering/_packaging/${marker}@Release/npm/registry/x`,
      `${INTERNAL}/npm/registry/${marker}%2fx`,
      `${SHARED}/pypi/simple/../${marker}/`,
      `${SHARED}/${marker}/x`,
      `/contoso/_apis/${marker}`,
      `${SHARED}/pypi/simple/${marker}%zz/`,
    ]) {
      const decision = check(target);
      expect(decision.allow).toBe(false);
      if (!decision.allow) expect(decision.detail).not.toContain(marker);
    }
  });
});

describe("isAllowedRedirectHost", () => {
  it("accepts hosts under a catalogued suffix", () => {
    expect(isAllowedRedirectHost("account.blob.core.windows.net")).toBe(true);
    expect(isAllowedRedirectHost("a.b.vsblob.visualstudio.com")).toBe(true);
    expect(isAllowedRedirectHost("lylvsblobprodcus31.vsblob.vsassets.io")).toBe(true);
    expect(isAllowedRedirectHost("ACCOUNT.BLOB.CORE.WINDOWS.NET.")).toBe(true);
  });

  it("rejects look-alikes and the bare suffix", () => {
    expect(isAllowedRedirectHost("evil.blob.core.windows.net.attacker.test")).toBe(false);
    expect(isAllowedRedirectHost("evilblob.core.windows.net")).toBe(false);
    expect(isAllowedRedirectHost("blob.core.windows.net")).toBe(false);
    expect(isAllowedRedirectHost(".blob.core.windows.net")).toBe(false);
    expect(isAllowedRedirectHost("vsblob.visualstudio.com")).toBe(false);
    expect(isAllowedRedirectHost("vsblob.vsassets.io")).toBe(false);
    expect(isAllowedRedirectHost("cdn.vsassets.io")).toBe(false);
    expect(isAllowedRedirectHost("x.vsblob.vsassets.io.attacker.test")).toBe(false);
    expect(isAllowedRedirectHost("[::1]")).toBe(false);
    expect(isAllowedRedirectHost("")).toBe(false);
  });
});

describe("authorizePackageRedirect", () => {
  const redirect = (location: string | undefined) =>
    authorizePackageRedirect(location, HOST, POLICY);

  it("relays a signed blob URL", () => {
    const location = "https://account.blob.core.windows.net/c/blob?sv=2020&sig=abc%2B";
    expect(redirect(location)).toEqual({ allow: true, location });
    expect(redirect("https://x.vsblob.visualstudio.com/a/b")).toMatchObject({ allow: true });
    expect(
      redirect("https://lylvsblobprodcus31.vsblob.vsassets.io/b-1/x?sv=2019&sig=abc"),
    ).toMatchObject({ allow: true });
  });

  it("refuses look-alike, non-https, credentialed, and off-port blob URLs", () => {
    for (const location of [
      "https://evil.blob.core.windows.net.attacker.test/x",
      "https://blob.core.windows.net/x",
      "http://account.blob.core.windows.net/x",
      "https://user:pass@account.blob.core.windows.net/x",
      "https://user@account.blob.core.windows.net/x",
      "https://account.blob.core.windows.net:8443/x",
      "ftp://account.blob.core.windows.net/x",
      "javascript:alert(1)",
      "https://attacker.test/?u=account.blob.core.windows.net",
    ]) {
      expect(redirect(location), location).toMatchObject({ allow: false });
    }
  });

  it("refuses a missing, empty, or malformed Location", () => {
    expect(redirect(undefined)).toMatchObject({ allow: false });
    expect(redirect("")).toMatchObject({ allow: false });
    expect(redirect("https://account.blob.core.windows.net/a\r\nSet-Cookie: x")).toMatchObject({
      allow: false,
    });
    expect(redirect("https://[not-an-ip/")).toMatchObject({ allow: false });
  });

  it("relays a same-host redirect only to an authorized route", () => {
    expect(redirect(`https://${HOST}${SHARED}/pypi/download/x/1.0/x.whl`)).toMatchObject({
      allow: true,
    });
    expect(redirect(`${SHARED}/pypi/simple/x/`)).toEqual({
      allow: true,
      location: `https://${HOST}${SHARED}/pypi/simple/x/`,
    });
    expect(redirect(`https://${HOST}/contoso/_apis/packaging/feeds`)).toMatchObject({
      allow: false,
    });
    expect(redirect(`https://${HOST}/contoso/_packaging/other/pypi/simple/x/`)).toMatchObject({
      allow: false,
    });
    expect(redirect(`https://${HOST}:8443${SHARED}/pypi/simple/x/`)).toMatchObject({
      allow: false,
    });
    expect(redirect(`http://${HOST}${SHARED}/pypi/simple/x/`)).toMatchObject({ allow: false });
  });

  it("does not treat another protected host as same-host", () => {
    expect(redirect("https://dev.azure.com/contoso/_apis/projects")).toMatchObject({
      allow: false,
    });
  });

  it("never echoes the Location into the denial detail", () => {
    const decision = redirect("https://secretmarker.attacker.test/x");
    expect(decision.allow).toBe(false);
    if (!decision.allow) expect(decision.detail).not.toContain("secretmarker");
  });
});

describe("packageAuthorizationHeader", () => {
  const route = (protocol: string) => {
    const found = PACKAGE_PROTOCOLS.find((entry) => entry.protocol === protocol);
    if (found === undefined) throw new Error(`no ${protocol} route`);
    return found;
  };

  it("sends a bearer for npm and cargo", () => {
    expect(packageAuthorizationHeader(route("npm"), "tok")).toBe("Bearer tok");
    expect(packageAuthorizationHeader(route("cargo"), "tok")).toBe("Bearer tok");
  });

  it("sends Basic with a fixed user for NuGet and PyPI", () => {
    const expected = `Basic ${Buffer.from("ado-aw:tok").toString("base64")}`;
    expect(packageAuthorizationHeader(route("nuget"), "tok")).toBe(expected);
    expect(packageAuthorizationHeader(route("pypi"), "tok")).toBe(expected);
  });
});
