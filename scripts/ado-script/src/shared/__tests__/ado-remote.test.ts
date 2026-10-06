import { describe, expect, it } from "vitest";

import {
  adoOrganizationFromCollectionUri,
  isCurrentAdoOrganization,
  normalizeAdoOrganizationUrl,
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
    expect(
      adoOrganizationFromCollectionUri("http://myorg.visualstudio.com/"),
    ).toBeNull();
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
  it("keeps discovery suffixes without treating discovery URLs as collection identity", () => {
    const legacy = "https://org.visualstudio.com/DefaultCollection/_apis/resourceAreas?api-version=7.1#area";
    const normalized = normalizeAdoOrganizationUrl(legacy);
    expect(normalized?.canonicalUrl.toString()).toBe(
      "https://dev.azure.com/org/_apis/resourceAreas?api-version=7.1#area",
    );
    const env = {
      BUILD_REASON: "PullRequest",
      BUILD_REPOSITORY_PROVIDER: "TfsGit",
      BUILD_REPOSITORY_URI: "https://org.visualstudio.com/DefaultCollection/Project/_git/repo",
      BUILD_REPOSITORY_ID: "11111111-1111-1111-1111-111111111111",
      SYSTEM_PULLREQUEST_PULLREQUESTID: "42",
      SYSTEM_COLLECTIONURI: "https://org.visualstudio.com/DefaultCollection/",
    };
    expect(nativeTriggeringPrIdentity(env)?.id).toBe("42");
    for (const collection of [legacy, normalized!.canonicalUrl.toString(), "http://org.visualstudio.com/"]) {
      expect(adoOrganizationFromCollectionUri(collection)).toBeNull();
      expect(nativeTriggeringPrIdentity({ ...env, SYSTEM_COLLECTIONURI: collection })).toBeUndefined();
    }
  });

  it("retains strict decoded-segment validation for both URL consumers", () => {
    for (const segment of ["org%2Fother", "org%5Cother", "org%00", "org%7F", "%ZZ"]) {
      expect(normalizeAdoOrganizationUrl(`https://dev.azure.com/${segment}/_apis/resourceAreas`)).toBeNull();
      expect(parseAdoRepoUrl(`https://dev.azure.com/${segment}/Project/_git/repo`)).toBeNull();
      expect(adoOrganizationFromCollectionUri(`https://dev.azure.com/${segment}/`)).toBeNull();
    }
  });

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
