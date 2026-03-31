import { QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { getVersion } from "../api/system";
import { AppRouter } from "./router";
import { createTestQueryClient } from "../test/query-client";

vi.mock("@xterm/xterm", () => ({
  Terminal: class {
    cols = 120;
    rows = 32;

    clear() {}
    dispose() {}
    loadAddon() {}
    open() {}
    onData() {
      return { dispose() {} };
    }
    write() {}
    writeln() {}
  },
}));

vi.mock("@xterm/addon-fit", () => ({
  FitAddon: class {
    fit() {}
  },
}));

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
    expect(
      await screen.findByRole("link", { name: "temporary run POC" }),
    ).toHaveAttribute("href", "/run-poc");
    expect(
      await screen.findByRole("link", { name: "temporary terminal POC" }),
    ).toHaveAttribute("href", "/terminal-poc");
  });

  it("renders the temporary run route", () => {
    render(
      <QueryClientProvider client={createTestQueryClient()}>
        <MemoryRouter initialEntries={["/run-poc"]}>
          <AppRouter />
        </MemoryRouter>
      </QueryClientProvider>,
    );

    expect(
      screen.getByRole("heading", {
        name: "Direct upload, async runs, and bounded bulk ingestion.",
      }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Start Run" }),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/polls many runs after a bulk upload batch/i),
    ).toBeInTheDocument();
  });

  it("renders the temporary terminal route", () => {
    render(
      <QueryClientProvider client={createTestQueryClient()}>
        <MemoryRouter initialEntries={["/terminal-poc"]}>
          <AppRouter />
        </MemoryRouter>
      </QueryClientProvider>,
    );

    expect(
      screen.getByRole("heading", {
        name: "Interactive shell over the session bridge.",
      }),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Connect" })).toBeInTheDocument();
    expect(
      screen.getByText(/Sessions hard-stop after about 220 seconds/),
    ).toBeInTheDocument();
  });
});
