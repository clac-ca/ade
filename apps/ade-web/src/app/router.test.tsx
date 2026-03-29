import { QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { getVersion } from "../api/system";
import { AppRouter } from "./router";
import { createTestQueryClient } from "../test/query-client";

vi.mock("../api/system", () => ({
  getVersion: vi.fn(),
}));

afterEach(() => {
  vi.mocked(getVersion).mockReset();
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
    vi.mocked(getVersion).mockResolvedValue({
      service: "ade",
      version: "0.1.0",
    });

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
    expect(await screen.findByText("0.1.0")).toBeInTheDocument();
    expect(getVersion).toHaveBeenCalledTimes(1);
  });
});
