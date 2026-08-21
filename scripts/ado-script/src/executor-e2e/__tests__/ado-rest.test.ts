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
