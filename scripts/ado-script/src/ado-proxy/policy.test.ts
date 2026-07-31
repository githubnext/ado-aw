import { describe, expect, it } from "vitest";

import { CATALOG_SCHEMA_VERSION } from "./catalog.js";
import type { ProxyPolicy } from "./config.js";
import { authorize, type Decision } from "./policy.js";
import { normalizeTarget } from "./route.js";

const POLICY: ProxyPolicy = {
  catalog_version: CATALOG_SCHEMA_VERSION,
  organization: "contoso",
  project: "Widgets",
  project_id: "11111111-1111-1111-1111-111111111111",
  repository: "widget-api",
  repository_id: "22222222-2222-2222-2222-222222222222",
  capabilities: ["discovery", "core", "repos", "pipelines", "boards"],
  protected_hosts: ["dev.azure.com", "app.vssps.visualstudio.com"],
  allowed_resource_areas: ["79134c72-4a58-4b42-976c-04e7115f32bf"],
};

function decide(
  method: string,
  url: string,
  options: { host?: string; accept?: string; policy?: ProxyPolicy } = {},
): Decision {
  return authorize(
    {
      method,
      host: options.host ?? "dev.azure.com",
      target: normalizeTarget(url),
      accept: options.accept,
    },
    options.policy ?? POLICY,
  );
}

function expectDeny(decision: Decision, reason: string): void {
  expect(decision.allow).toBe(false);
  if (decision.allow) return;
  expect(decision.reason).toBe(reason);
}

describe("authorize — allowed reads", () => {
  it("allows the current project", () => {
    const decision = decide("GET", "/contoso/_apis/projects/Widgets?api-version=7.1");
    expect(decision.allow).toBe(true);
    if (decision.allow) expect(decision.operation.id).toBe("core.project-get");
  });

  it("accepts the project GUID as well as the name", () => {
    // `az` substitutes whichever identifier it cached, so both must work — but
    // only for the pinned project.
    expect(
      decide(
        "GET",
        `/contoso/_apis/projects/${POLICY.project_id as string}?api-version=7.1`,
      ).allow,
    ).toBe(true);
  });

  it("allows discovery OPTIONS without an api-version", () => {
    expect(decide("OPTIONS", "/contoso/_apis").allow).toBe(true);
  });

  it("allows a repository read in the current project", () => {
    const decision = decide(
      "GET",
      "/contoso/Widgets/_apis/git/repositories/widget-api/refs?api-version=7.1&filter=heads",
    );
    expect(decision.allow).toBe(true);
  });

  it("allows the SPS resource-area fallback for an allowed area", () => {
    expect(
      decide(
        "GET",
        "/_apis/resourceareas/79134c72-4a58-4b42-976c-04e7115f32bf?api-version=7.1",
        { host: "app.vssps.visualstudio.com" },
      ).allow,
    ).toBe(true);
  });

  it("reads the api-version from the Accept header", () => {
    expect(
      decide("GET", "/contoso/_apis/projects/Widgets", {
        accept: "application/json;api-version=7.1;excludeUrls=true",
      }).allow,
    ).toBe(true);
  });
});

describe("authorize — denials", () => {
  it("denies every non-read method", () => {
    for (const method of ["POST", "PUT", "PATCH", "DELETE", "HEAD", "TRACE"]) {
      expectDeny(decide(method, "/contoso/_apis/projects/Widgets?api-version=7.1"), "method-not-read");
    }
  });

  it("denies an unknown host", () => {
    expectDeny(
      decide("GET", "/contoso/_apis/projects/Widgets?api-version=7.1", {
        host: "evil.test",
      }),
      "unknown-host",
    );
  });

  it("denies credential-bearing route families outright", () => {
    for (const path of [
      "/contoso/Widgets/_apis/serviceendpoint/endpoints?api-version=7.1",
      "/contoso/Widgets/_apis/distributedtask/variablegroups?api-version=7.1",
      "/contoso/Widgets/_apis/distributedtask/securefiles?api-version=7.1",
      "/contoso/Widgets/_git/widget-api/info/refs?service=git-upload-pack",
    ]) {
      expectDeny(decide("GET", path), "denied-route-family");
    }
  });

  it("denies placeholder-bearing families such as the build OAuth token", () => {
    // These are the families defence-in-depth exists for: they are not in the
    // allowlist either, but a future catalog mistake must not make them
    // reachable.
    for (const path of [
      "/contoso/Widgets/_apis/build/builds/42/oauthtoken?api-version=7.1",
      "/contoso/Widgets/_apis/build/builds/42/artifacts?api-version=7.1",
      "/contoso/Widgets/_apis/build/builds/42/logs?api-version=7.1",
      "/contoso/Widgets/_apis/git/repositories/widget-api/blobs?api-version=7.1",
      "/contoso/Widgets/_apis/git/repositories/widget-api/itemsbatch?api-version=7.1",
    ]) {
      expectDeny(decide("GET", path), "denied-route-family");
    }
  });

  it("denies the batch work-item read while leaving the single read reachable", () => {
    expectDeny(
      decide("GET", "/contoso/_apis/wit/workitems?ids=1,2,3&api-version=7.1"),
      "denied-route-family",
    );
    expect(decide("GET", "/contoso/_apis/wit/workitems/42?api-version=7.1").allow).toBe(
      true,
    );
  });

  it("denies an uncatalogued route", () => {
    expectDeny(
      decide("GET", "/contoso/_apis/graph/users?api-version=7.1"),
      "unknown-route",
    );
  });

  it("denies an operation whose capability is disabled", () => {
    const decision = decide(
      "GET",
      "/contoso/Widgets/_apis/wit/workitems/42?api-version=7.1",
      { policy: { ...POLICY, capabilities: ["discovery", "core"] } },
    );
    expectDeny(decision, "capability-disabled");
  });

  it("denies a different organization", () => {
    expectDeny(
      decide("GET", "/fabrikam/_apis/projects/Widgets?api-version=7.1"),
      "out-of-scope",
    );
  });

  it("denies a different project", () => {
    expectDeny(
      decide("GET", "/contoso/_apis/projects/Secrets?api-version=7.1"),
      "out-of-scope",
    );
    expectDeny(
      decide(
        "GET",
        "/contoso/Secrets/_apis/build/builds?api-version=7.1",
      ),
      "out-of-scope",
    );
  });

  it("denies a different repository in the current project", () => {
    expectDeny(
      decide(
        "GET",
        "/contoso/Widgets/_apis/git/repositories/other-repo/items?api-version=7.1",
      ),
      "out-of-scope",
    );
  });

  it("denies a resource area outside the allowed set", () => {
    expectDeny(
      decide(
        "GET",
        "/_apis/resourceareas/99999999-9999-9999-9999-999999999999?api-version=7.1",
        { host: "app.vssps.visualstudio.com" },
      ),
      "out-of-scope",
    );
  });

  it("denies an unlisted query parameter", () => {
    // Unknown parameters change what an endpoint returns; `$expand` in
    // particular can pull in fields the catalog never reviewed.
    expectDeny(
      decide("GET", "/contoso/_apis/projects/Widgets?api-version=7.1&$expand=all"),
      "query-not-allowed",
    );
  });

  it("denies a missing, conflicting, or out-of-range api-version", () => {
    expectDeny(decide("GET", "/contoso/_apis/projects/Widgets"), "api-version");
    expectDeny(
      decide("GET", "/contoso/_apis/projects/Widgets?api-version=1.0"),
      "api-version",
    );
    expectDeny(
      decide("GET", "/contoso/_apis/projects/Widgets?api-version=7.1", {
        accept: "application/json;api-version=3.0",
      }),
      "api-version",
    );
  });

  it("denies an api-version on a discovery OPTIONS", () => {
    expectDeny(decide("OPTIONS", "/contoso/_apis?api-version=7.1"), "api-version");
  });

  it("reports a capability denial only when nothing else matches", () => {
    // `core` is enabled here, so the project read must still be allowed even
    // though `boards` is off.
    const decision = decide("GET", "/contoso/_apis/projects/Widgets?api-version=7.1", {
      policy: { ...POLICY, capabilities: ["discovery", "core"] },
    });
    expect(decision.allow).toBe(true);
  });
});

describe("authorize — response-scoped operations", () => {
  it("allows the org-level pull-request read that az repos pr show needs", () => {
    // Its URL carries no project or repository, so the scope check happens on
    // the response body instead.
    const decision = decide("GET", "/contoso/_apis/git/pullrequests/7?api-version=7.1");
    expect(decision.allow).toBe(true);
    if (decision.allow) {
      expect(decision.operation.response).toBe("validate-project-and-repository");
    }
  });

  it("allows the org-level work-item read that az boards work-item show needs", () => {
    const decision = decide("GET", "/contoso/_apis/wit/workitems/42?api-version=7.1");
    expect(decision.allow).toBe(true);
    if (decision.allow) expect(decision.operation.response).toBe("validate-project");
  });
});
