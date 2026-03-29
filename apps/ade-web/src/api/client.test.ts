import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiError, apiFetch, apiRequest } from "./client";

afterEach(() => {
  vi.restoreAllMocks();
});

describe("apiFetch", () => {
  it("normalizes relative API paths against /api", async () => {
    const fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      json: async () => ({ ok: true }),
    });

    vi.stubGlobal("fetch", fetchMock);

    await apiFetch("/version");

    expect(fetchMock).toHaveBeenCalledWith(
      "/api/version",
      expect.objectContaining({
        headers: expect.any(Headers),
      }),
    );
  });

  it("throws ApiError with the response message when a request fails", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue({
        ok: false,
        status: 503,
        json: async () => ({
          message: "Service unavailable",
        }),
      }),
    );

    await expect(apiFetch("/version")).rejects.toEqual(
      new ApiError("Service unavailable", 503),
    );
  });

  it("returns the raw response for non-JSON callers", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(
        new Response("plain text", {
          headers: { "content-type": "text/plain" },
          status: 200,
        }),
      ),
    );

    const response = await apiRequest("/files/content/notes.txt");

    expect(await response.text()).toBe("plain text");
  });
});
