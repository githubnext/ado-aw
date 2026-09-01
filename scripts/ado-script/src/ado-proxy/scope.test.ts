import { describe, expect, it } from "vitest";

import { CATALOG_SCHEMA_VERSION } from "./catalog.js";
import type { ProxyPolicy } from "./config.js";
import { ScopeIndex } from "./scope.js";

const POLICY: ProxyPolicy = {
  catalog_version: CATALOG_SCHEMA_VERSION,
  organization: "contoso",
  project: "Current",
  project_id: "11111111-1111-1111-1111-111111111111",
  repository: "current-repo",
  repository_id: "22222222-2222-2222-2222-222222222222",
  additional_scopes: [
    {
      organization: "fabrikam",
      projects: [
        {
          project: "Shared",
          project_id: "33333333-3333-3333-3333-333333333333",
          project_scoped: true,
          repositories: ["shared-api"],
        },
      ],
    },
    {
      organization: "contoso",
      projects: [
        {
          project: "RepoOnly",
          project_scoped: false,
          repositories: ["checked-out-repo"],
        },
      ],
    },
  ],
  capabilities: ["discovery", "core", "repos"],
  protected_hosts: ["dev.azure.com", "app.vssps.visualstudio.com"],
  allowed_resource_areas: [],
};

describe("ScopeIndex", () => {
  it("seeds the current scope by both names and ids", () => {
    const scopes = ScopeIndex.from(POLICY);

    expect(scopes.allowsProject("contoso", "Current")).toBe(true);
    expect(scopes.allowsProject("CONTOSO", POLICY.project_id)).toBe(true);
    expect(scopes.allowsRepository("contoso", "Current", "current-repo")).toBe(true);
    expect(
      scopes.allowsRepository("contoso", POLICY.project_id, POLICY.repository_id),
    ).toBe(true);
  });

  it("resolves project grants within their organization, never globally", () => {
    const scopes = ScopeIndex.from(POLICY);

    expect(scopes.allowsProject("fabrikam", "Shared")).toBe(true);
    expect(
      scopes.allowsProject("fabrikam", "33333333-3333-3333-3333-333333333333"),
    ).toBe(true);

    // The load-bearing negative: a flat "is Shared allowed anywhere?" check
    // would return true here and silently widen scope across organizations.
    expect(scopes.allowsProject("contoso", "Shared")).toBe(false);
    expect(scopes.allowsRepository("contoso", "Shared", "shared-api")).toBe(false);
  });

  it("grants a repos-derived repository without granting its project", () => {
    const scopes = ScopeIndex.from(POLICY);

    expect(scopes.allowsRepository("contoso", "RepoOnly", "checked-out-repo")).toBe(
      true,
    );
    expect(scopes.allowsProject("contoso", "RepoOnly")).toBe(false);
  });

  it("merges duplicate grants without allowing a later entry to revoke scope", () => {
    const scopes = ScopeIndex.from({
      ...POLICY,
      additional_scopes: [
        {
          organization: "fabrikam",
          projects: [
            {
              project: "Shared",
              project_scoped: true,
              repositories: ["one"],
            },
            {
              project: "Shared",
              project_scoped: false,
              repositories: ["two"],
            },
          ],
        },
      ],
    });

    expect(scopes.allowsProject("fabrikam", "Shared")).toBe(true);
    expect(scopes.allowsRepository("fabrikam", "Shared", "one")).toBe(true);
    expect(scopes.allowsRepository("fabrikam", "Shared", "two")).toBe(true);
  });
});
