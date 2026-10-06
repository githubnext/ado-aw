import { describe, expect, it } from "vitest";

import {
  adoOrganizationFromCollectionUri,
  isCurrentAdoOrganization,
  normalizeAdoOrganizationUrl,
  parseAdoRepoUrl,
} from "../ado-remote.js";

describe("parseAdoRepoUrl", () => {
  it("parses dev.azure.com remotes with userinfo and encoded names", () => {
    expect(
      parseAdoRepoUrl(
        "https://build@dev.azure.com/MyOrg/My%20Project/_git/repo%20name",
      ),
    ).toEqual({
      collectionUri: "https://dev.azure.com/MyOrg/",
      organization: "myorg",
      project: "My Project",
      repository: "repo name",
    });
  });

  it("parses visualstudio.com remotes", () => {
    expect(
      parseAdoRepoUrl("https://myorg.visualstudio.com/Project/_git/repo/"),
    ).toEqual({
      collectionUri: "https://myorg.visualstudio.com/",
      organization: "myorg",
      project: "Project",
      repository: "repo",
    });
  });

  it("parses legacy DefaultCollection visualstudio.com remotes", () => {
    expect(
      parseAdoRepoUrl(
        "https://myorg.visualstudio.com/DefaultCollection/Project/_git/repo",
      ),
    ).toEqual({
      collectionUri: "https://myorg.visualstudio.com/DefaultCollection/",
      organization: "myorg",
      project: "Project",
      repository: "repo",
    });
  });

  it("rejects non-ADO and malformed remotes", () => {
    expect(parseAdoRepoUrl("https://github.com/org/repo.git")).toBeNull();
    expect(parseAdoRepoUrl("not a url")).toBeNull();
    expect(parseAdoRepoUrl("https://dev.azure.com/org/project/repo")).toBeNull();
  });
});

describe("ADO collection matching", () => {
  it("extracts organizations from both service URL forms", () => {
    expect(adoOrganizationFromCollectionUri("https://dev.azure.com/MyOrg/")).toBe(
      "myorg",
    );
    expect(
      adoOrganizationFromCollectionUri("https://myorg.visualstudio.com/"),
    ).toBe("myorg");
    expect(
      adoOrganizationFromCollectionUri("http://myorg.visualstudio.com/"),
    ).toBe("myorg");
  });

  it("recognizes same-org identities and rejects cross-org identities", () => {
    const identity = parseAdoRepoUrl(
      "https://dev.azure.com/myorg/Project/_git/repo",
    )!;
    expect(
      isCurrentAdoOrganization(identity, {
        SYSTEM_COLLECTIONURI: "https://dev.azure.com/myorg/",
      }),
    ).toBe(true);
    expect(
      isCurrentAdoOrganization(identity, {
        SYSTEM_COLLECTIONURI: "https://dev.azure.com/other/",
      }),
    ).toBe(false);
  });
});

describe("normalizeAdoOrganizationUrl", () => {
  it("preserves modern organization URLs", () => {
    const normalized = normalizeAdoOrganizationUrl(
      "https://dev.azure.com/My%20Org/sub/path?api-version=7.1#area",
    );
    expect(normalized?.organization).toBe("my org");
    expect(normalized?.canonicalUrl.toString()).toBe(
      "https://dev.azure.com/My%20Org/sub/path?api-version=7.1#area",
    );
  });

  it("moves a legacy hostname organization into the canonical path", () => {
    const normalized = normalizeAdoOrganizationUrl(
      "https://Contoso.visualstudio.com/service/path?x=1#fragment",
    );
    expect(normalized?.organization).toBe("contoso");
    expect(normalized?.canonicalUrl.toString()).toBe(
      "https://dev.azure.com/contoso/service/path?x=1#fragment",
    );
  });

  it("removes exactly one legacy DefaultCollection segment", () => {
    const nested = normalizeAdoOrganizationUrl(
      "https://contoso.visualstudio.com/DEFAULTCOLLECTION/service/path",
    );
    expect(nested?.organization).toBe("contoso");
    expect(nested?.canonicalUrl.toString()).toBe(
      "https://dev.azure.com/contoso/service/path",
    );

    const root = normalizeAdoOrganizationUrl(
      "https://contoso.visualstudio.com/DefaultCollection/",
    );
    expect(root?.organization).toBe("contoso");
    expect(root?.canonicalUrl.toString()).toBe(
      "https://dev.azure.com/contoso/",
    );
  });

  it("rejects unrelated, insecure, organization-less, and malformed URLs", () => {
    expect(normalizeAdoOrganizationUrl("https://example.test/contoso/")).toBeNull();
    expect(
      normalizeAdoOrganizationUrl("https://app.vssps.visualstudio.com/"),
    ).toBeNull();
    expect(normalizeAdoOrganizationUrl("http://dev.azure.com/contoso/")).toBeNull();
    expect(normalizeAdoOrganizationUrl("https://dev.azure.com/")).toBeNull();
    expect(normalizeAdoOrganizationUrl("https://visualstudio.com/")).toBeNull();
    expect(normalizeAdoOrganizationUrl("not a url")).toBeNull();
  });
});
