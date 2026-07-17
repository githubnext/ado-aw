import { describe, expect, it } from "vitest";

import {
  adoOrganizationFromCollectionUri,
  isCurrentAdoOrganization,
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
