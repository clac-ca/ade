import { QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AppRouter } from "./router";
import { createTestQueryClient } from "../test/query-client";

afterEach(() => {
  vi.restoreAllMocks();
});

describe("AppRouter", () => {
  it("renders the not-found page for unknown routes", () => {
    render(
      <QueryClientProvider client={createTestQueryClient()}>
        <MemoryRouter initialEntries={["/missing"]}>
          <AppRouter />
        </MemoryRouter>
      </QueryClientProvider>,
    );

    expect(
      screen.getByRole("heading", { name: "This route does not exist." }),
    ).toBeInTheDocument();
  });

  it("renders the home page and loads version metadata", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue({
        ok: true,
        json: async () => ({
          builtAt: "2026-03-26T00:00:00.000Z",
          gitSha: "abc123",
          runtimeVersion: "rustc 1.94.1",
          service: "ade",
          version: "0.1.0",
        }),
      }),
    );

    render(
      <QueryClientProvider client={createTestQueryClient()}>
        <MemoryRouter initialEntries={["/"]}>
          <AppRouter />
        </MemoryRouter>
      </QueryClientProvider>,
    );

    expect(
      await screen.findByRole("heading", {
        name: "The frontend stays deliberately small and same-origin.",
      }),
    ).toBeInTheDocument();
    expect(await screen.findByText("rustc 1.94.1")).toBeInTheDocument();
  });
});
