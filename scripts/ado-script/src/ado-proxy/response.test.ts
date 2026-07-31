import { describe, expect, it } from "vitest";

import { CATALOG_SCHEMA_VERSION, OPERATIONS } from "./catalog.js";
import type { ProxyPolicy } from "./config.js";
import { filterResponse, isProtectedLocation } from "./response.js";
import type { Operation } from "../shared/ado-proxy-catalog.types.gen.js";

const POLICY: ProxyPolicy = {
  catalog_version: CATALOG_SCHEMA_VERSION,
  organization: "contoso",
  project: "Widgets",
  project_id: "11111111-1111-1111-1111-111111111111",
  repository: "widget-api",
  repository_id: "22222222-2222-2222-2222-222222222222",
  capabilities: ["discovery", "core", "repos", "pipelines", "boards"],
  protected_hosts: ["dev.azure.com", "app.vssps.visualstudio.com"],
  allowed_resource_areas: [],
};

/** The real catalog entry, so these tests break if a response policy moves. */
function operation(id: string): Operation {
  const found = OPERATIONS.find((entry) => entry.id === id);
  if (found === undefined) throw new Error(`no catalog operation ${id}`);
  return found;
}

function apply(id: string, document: unknown): ReturnType<typeof filterResponse> {
  return filterResponse(
    operation(id),
    POLICY,
    Buffer.from(JSON.stringify(document), "utf8"),
  );
}

function forwarded(outcome: ReturnType<typeof filterResponse>): unknown {
  expect(outcome.kind).toBe("forward");
  if (outcome.kind !== "forward") throw new Error("expected forward");
  return JSON.parse(outcome.body.toString("utf8"));
}

describe("filterResponse — pass-through", () => {
  it("forwards a plain JSON operation byte-for-byte", () => {
    const body = Buffer.from('{"id":"not even valid for this shape"}', "utf8");
    const outcome = filterResponse(operation("core.project-get"), POLICY, body);
    expect(outcome.kind).toBe("forward");
    if (outcome.kind === "forward") expect(outcome.body.equals(body)).toBe(true);
  });
});

describe("filterResponse — project list", () => {
  it("narrows the list to the pinned project", () => {
    // `az devops` lists projects during credential validation; the agent must
    // not learn which other projects exist in the organization.
    const outcome = apply("core.project-validation-probe", {
      count: 3,
      value: [
        { id: "11111111-1111-1111-1111-111111111111", name: "Widgets" },
        { id: "33333333-3333-3333-3333-333333333333", name: "Secrets" },
        { id: "44444444-4444-4444-4444-444444444444", name: "Payroll" },
      ],
    });
    expect(forwarded(outcome)).toEqual({
      count: 1,
      value: [{ id: "11111111-1111-1111-1111-111111111111", name: "Widgets" }],
    });
  });

  it("returns an empty list rather than failing when nothing matches", () => {
    expect(forwarded(apply("core.project-validation-probe", { count: 1, value: [{ name: "Other" }] })))
      .toEqual({ count: 0, value: [] });
  });

  it("denies a list envelope it cannot parse", () => {
    const outcome = filterResponse(
      operation("core.project-validation-probe"),
      POLICY,
      Buffer.from("<html>sign in</html>", "utf8"),
    );
    expect(outcome.kind).toBe("deny");
  });
});

describe("filterResponse — resource areas", () => {
  it("drops areas that point outside the protected set", () => {
    // A retained entry would send the client's next call to a host this proxy
    // does not police.
    const outcome = apply("discovery.resource-areas", {
      count: 2,
      value: [
        { id: "a", locationUrl: "https://dev.azure.com/contoso/" },
        { id: "b", locationUrl: "https://vsrm.dev.azure.com/contoso/" },
      ],
    });
    expect(forwarded(outcome)).toEqual({
      count: 1,
      value: [{ id: "a", locationUrl: "https://dev.azure.com/contoso/" }],
    });
  });
});

describe("filterResponse — response-scoped validation", () => {
  it("allows a work item in the pinned project", () => {
    expect(
      apply("boards.work-item-get-by-id", {
        id: 42,
        fields: { "System.TeamProject": "Widgets" },
      }).kind,
    ).toBe("forward");
  });

  it("denies a work item in another project", () => {
    // The URL is organization-scoped, so this is the only place the scope can
    // be enforced.
    const outcome = apply("boards.work-item-get-by-id", {
      id: 42,
      fields: { "System.TeamProject": "Secrets" },
    });
    expect(outcome.kind).toBe("deny");
  });

  it("denies a work item that reports no project at all", () => {
    expect(apply("boards.work-item-get-by-id", { id: 42 }).kind).toBe("deny");
  });

  it("allows a pull request in the pinned project and repository", () => {
    expect(
      apply("repos.pull-request-get-by-id", {
        pullRequestId: 7,
        repository: { name: "widget-api", project: { name: "Widgets" } },
      }).kind,
    ).toBe("forward");
  });

  it("denies a pull request in another repository of the same project", () => {
    const outcome = apply("repos.pull-request-get-by-id", {
      pullRequestId: 7,
      repository: { name: "other-repo", project: { name: "Widgets" } },
    });
    expect(outcome.kind).toBe("deny");
  });

  it("denies a pull request in another project", () => {
    const outcome = apply("repos.pull-request-get-by-id", {
      pullRequestId: 7,
      repository: { name: "widget-api", project: { name: "Secrets" } },
    });
    expect(outcome.kind).toBe("deny");
  });

  it("denies a pull request response with no repository to validate", () => {
    expect(apply("repos.pull-request-get-by-id", { pullRequestId: 7 }).kind).toBe("deny");
  });
});

describe("isProtectedLocation", () => {
  it("accepts protected hosts and rejects everything else", () => {
    expect(isProtectedLocation("https://dev.azure.com/contoso/")).toBe(true);
    expect(isProtectedLocation("https://vssps.dev.azure.com/contoso/")).toBe(false);
    expect(isProtectedLocation("not a url")).toBe(false);
    expect(isProtectedLocation(undefined)).toBe(false);
  });
});
