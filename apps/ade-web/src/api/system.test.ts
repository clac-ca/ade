import { describe, expect, it, vi } from "vitest";
import { ApiError } from "./client";
import { getVersion, type VersionClient } from "./system";

function mockClient(
  result: Awaited<ReturnType<VersionClient["GET"]>>,
): {
  client: VersionClient;
  get: ReturnType<typeof vi.fn>;
} {
  const get = vi.fn().mockResolvedValue(result);

  return {
    client: {
      GET: get,
    },
    get,
  };
}

describe("getVersion", () => {
  it("returns version metadata on success", async () => {
    const { client, get } = mockClient({
      data: {
        service: "ade",
        version: "1.0.0",
      },
      response: new Response(null, { status: 200 }),
    });

    await expect(getVersion(client)).resolves.toEqual({
      service: "ade",
      version: "1.0.0",
    });
    expect(get).toHaveBeenCalledWith("/api/version", {});
  });

  it("uses the API error message when one is present", async () => {
    const { client } = mockClient({
      error: {
        message: "Service unavailable",
      },
      response: new Response(null, { status: 503 }),
    });

    await expect(getVersion(client)).rejects.toEqual(
      new ApiError("Service unavailable", 503),
    );
  });

  it("falls back to the HTTP status when the error body has no message", async () => {
    const { client } = mockClient({
      error: {
        code: "bad-request",
      },
      response: new Response(null, { status: 400 }),
    });

    await expect(getVersion(client)).rejects.toEqual(
      new ApiError("Request failed with status 400.", 400),
    );
  });
});
