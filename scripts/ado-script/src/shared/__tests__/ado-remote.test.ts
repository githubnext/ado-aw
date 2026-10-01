import { describe, expect, it } from "vitest";

import {
  adoOrganizationFromCollectionUri,
  isCurrentAdoOrganization,
  parseAdoRepoUrl,
  nativeTriggeringPrIdentity,
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
  it("captures equivalent native URL spellings but rejects malformed collection paths", () => {
    const env = {
      BUILD_REASON:"PullRequest", BUILD_REPOSITORY_PROVIDER:"TfsGit",
      BUILD_REPOSITORY_URI:"https://DEV.AZURE.COM/org/Other/_git/target/",
      BUILD_REPOSITORY_ID:"11111111-1111-1111-1111-111111111111",
      SYSTEM_PULLREQUEST_PULLREQUESTID:"18446744073709551615",
      SYSTEM_COLLECTIONURI:"https://org.visualstudio.com/DefaultCollection/",
    };
    expect(nativeTriggeringPrIdentity(env)?.id).toBe("18446744073709551615");
    for (const uri of ["https://dev.azure.com/org/extra","https://org.visualstudio.com/OtherCollection/"]) {
      expect(nativeTriggeringPrIdentity({...env,SYSTEM_COLLECTIONURI:uri})).toBeUndefined();
    }
    expect(nativeTriggeringPrIdentity({...env,BUILD_REPOSITORY_URI:"https://dev.azure.com/org//Other/_git/target"})).toBeUndefined();
  });
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
