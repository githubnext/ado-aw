import { afterEach, describe, expect, it, vi } from "vitest";

import { AdoRest } from "../ado-rest.js";

const options = {
  orgUrl: "https://dev.azure.com/org/",
  project: "My Project",
  token: "token",
};

function stubFetch(responder: (url: string) => Response): ReturnType<typeof vi.fn> {
  const fetchMock = vi.fn(async (url: string) => responder(String(url)));
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

describe("AdoRest.workItemTypeExists", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  describe("AdoRest.resolveIdentityId", () => {
    afterEach(() => {
      vi.unstubAllGlobals();
    });

    it("passes GUID identities through without a request", async () => {
      const fetchMock = vi.fn();
      vi.stubGlobal("fetch", fetchMock);

      await expect(
        new AdoRest(options).resolveIdentityId(
          "01234567-89ab-cdef-0123-456789abcdef",
        ),
      ).resolves.toBe("01234567-89ab-cdef-0123-456789abcdef");
      expect(fetchMock).not.toHaveBeenCalled();
    });

    it("encodes the identity query and accepts one case-insensitive exact match", async () => {
      const fetchMock = stubFetch(
        () =>
          new Response(
            JSON.stringify({
              value: [
                {
                  id: "reviewer-id",
                  displayName: "Near Match",
                  properties: {
                    Mail: { $value: "REQUESTER+E2E@example.com" },
                  },
                },
              ],
            }),
            {
              status: 200,
              headers: { "content-type": "application/json" },
            },
          ),
      );

      await expect(
        new AdoRest(options).resolveIdentityId("requester+e2e@example.com"),
      ).resolves.toBe("reviewer-id");
      expect(fetchMock.mock.calls[0]?.[0]).toBe(
        "https://vssps.dev.azure.com/org/_apis/identities?searchFilter=General&filterValue=requester%2Be2e%40example.com&api-version=7.1",
      );
    });

    it("rejects ambiguous exact matches", async () => {
      stubFetch(
        () =>
          new Response(
            JSON.stringify({
              value: [
                { id: "one", providerDisplayName: "owner@example.com" },
                {
                  id: "two",
                  properties: { Account: { $value: "OWNER@example.com" } },
                },
              ],
            }),
            {
              status: 200,
              headers: { "content-type": "application/json" },
            },
          ),
      );

      await expect(
        new AdoRest(options).resolveIdentityId("owner@example.com"),
      ).resolves.toBeUndefined();
    });
  });

  describe("AdoRest authentication", () => {
    afterEach(() => {
      vi.unstubAllGlobals();
    });

    it("uses Bearer auth when requested", async () => {
      const fetchMock = stubFetch(
        () =>
          new Response(JSON.stringify({ id: "repo" }), {
            status: 200,
            headers: { "content-type": "application/json" },
          }),
      );

      await new AdoRest({ ...options, authKind: "bearer" }).getRepository("repo");

      expect(fetchMock.mock.calls[0]?.[1]).toMatchObject({
        headers: expect.objectContaining({ Authorization: "Bearer token" }),
      });
    });
  });

  it("resolves true and encodes the project and type segments", async () => {
    const fetchMock = stubFetch(
      () =>
        new Response(JSON.stringify({ name: "Bug" }), {
          status: 200,
          headers: { "content-type": "application/json" },
        }),
    );

    await expect(new AdoRest(options).workItemTypeExists("User Story")).resolves.toBe(true);
    expect(fetchMock.mock.calls[0]?.[0]).toBe(
      "https://dev.azure.com/org/My%20Project/_apis/wit/workitemtypes/User%20Story?api-version=7.1",
    );
  });

  it("resolves false when the project does not define the type", async () => {
    stubFetch(() => new Response("not found", { status: 404 }));

    await expect(new AdoRest(options).workItemTypeExists("Bug")).resolves.toBe(false);
  });

  it("throws rather than reporting a missing type when the request fails", async () => {
    stubFetch(() => new Response("denied", { status: 403 }));

    await expect(new AdoRest(options).workItemTypeExists("Bug")).rejects.toThrow("HTTP 403");
  });
});
