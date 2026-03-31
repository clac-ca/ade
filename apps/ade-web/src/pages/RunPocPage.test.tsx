import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { RunPocPage } from "./RunPocPage";

type EventLike = {
  data?: string;
  lastEventId?: string;
};

class MockEventSource {
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSED = 2;
  static instances: MockEventSource[] = [];

  readonly listeners = new Map<string, Set<(event?: EventLike) => void>>();
  onerror: ((event?: EventLike) => void) | null = null;
  readyState = MockEventSource.OPEN;
  url: string;

  constructor(url: string) {
    this.url = url;
    MockEventSource.instances.push(this);
  }

  addEventListener(type: string, listener: (event?: EventLike) => void) {
    const listeners = this.listeners.get(type) ?? new Set();
    listeners.add(listener);
    this.listeners.set(type, listeners);
  }

  close() {
    this.readyState = MockEventSource.CLOSED;
  }

  emit(type: string, event?: EventLike) {
    for (const listener of this.listeners.get(type) ?? []) {
      listener(event);
    }
  }

  fail(event?: EventLike) {
    this.onerror?.(event);
  }
}

describe("RunPocPage", () => {
  beforeEach(() => {
    MockEventSource.instances = [];
    window.sessionStorage.clear();
    vi.stubGlobal("EventSource", MockEventSource);
  });

  afterEach(() => {
    cleanup();
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("ignores stale event sources after reconnecting", async () => {
    render(
      <MemoryRouter>
        <RunPocPage />
      </MemoryRouter>,
    );

    fireEvent.change(screen.getByLabelText("Run ID"), {
      target: { value: "run-1" },
    });

    fireEvent.click(screen.getByRole("button", { name: "Resume" }));
    const firstSource = MockEventSource.instances[0];
    expect(firstSource).toBeDefined();
    if (!firstSource) {
      throw new Error("missing first event source");
    }

    fireEvent.click(screen.getByRole("button", { name: "Resume" }));
    const secondSource = MockEventSource.instances[1];
    expect(secondSource).toBeDefined();
    if (!secondSource) {
      throw new Error("missing second event source");
    }

    expect(
      screen.getByText("Connecting to the run event stream..."),
    ).toBeInTheDocument();

    firstSource.emit("run.status", {
      data: JSON.stringify({
        phase: "installPackages",
        runId: "run-1",
        state: "started",
      }),
      lastEventId: "1",
    });

    expect(
      screen.queryByText("Run phase: installPackages."),
    ).not.toBeInTheDocument();

    secondSource.emit("run.status", {
      data: JSON.stringify({
        phase: "executeRun",
        runId: "run-1",
        state: "started",
      }),
      lastEventId: "2",
    });

    await waitFor(() => {
      expect(screen.getByText("Run phase: executeRun.")).toBeInTheDocument();
    });
    expect(screen.getByText("2")).toBeInTheDocument();
  });

  it("uploads a bulk batch directly to storage and polls run status", async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = typeof input === "string" ? input : input.toString();
      if (url.endsWith("/uploads/batches")) {
        return new Response(
          JSON.stringify({
            batchId: "bat_123",
            items: [
              {
                fileId: "fil_1",
                filePath:
                  "workspaces/workspace-a/configs/config-v1/uploads/batches/bat_123/fil_1/alpha.csv",
                upload: {
                  expiresAt: "2026-03-30T12:00:00Z",
                  headers: { "content-type": "text/csv" },
                  method: "PUT",
                  url: "https://blob.example.com/alpha.csv?sas",
                },
              },
              {
                fileId: "fil_2",
                filePath:
                  "workspaces/workspace-a/configs/config-v1/uploads/batches/bat_123/fil_2/beta.csv",
                upload: {
                  expiresAt: "2026-03-30T12:00:00Z",
                  headers: { "content-type": "text/csv" },
                  method: "PUT",
                  url: "https://blob.example.com/beta.csv?sas",
                },
              },
            ],
          }),
          {
            headers: { "Content-Type": "application/json" },
            status: 200,
          },
        );
      }

      if (url.endsWith("/runs") && !url.includes("/downloads")) {
        const nextRunId = fetchMock.mock.calls.filter(([candidate]) => {
          const candidateUrl =
            typeof candidate === "string" ? candidate : candidate.toString();
          return (
            candidateUrl.endsWith("/runs") &&
            !candidateUrl.includes("/downloads")
          );
        }).length;

        return new Response(
          JSON.stringify({
            runId: `run-${nextRunId}`,
            status: "pending",
          }),
          {
            headers: { "Content-Type": "application/json" },
            status: 202,
          },
        );
      }

      if (url.endsWith("/runs/run-1")) {
        return new Response(
          JSON.stringify({
            logPath:
              "workspaces/workspace-a/configs/config-v1/runs/run-1/logs/events.ndjson",
            outputPath:
              "workspaces/workspace-a/configs/config-v1/runs/run-1/output/normalized.xlsx",
            runId: "run-1",
            status: "succeeded",
          }),
          {
            headers: { "Content-Type": "application/json" },
            status: 200,
          },
        );
      }

      if (url.endsWith("/runs/run-2")) {
        return new Response(
          JSON.stringify({
            logPath:
              "workspaces/workspace-a/configs/config-v1/runs/run-2/logs/events.ndjson",
            outputPath:
              "workspaces/workspace-a/configs/config-v1/runs/run-2/output/normalized.xlsx",
            runId: "run-2",
            status: "succeeded",
          }),
          {
            headers: { "Content-Type": "application/json" },
            status: 200,
          },
        );
      }

      if (url.startsWith("https://blob.example.com/")) {
        return new Response(null, { status: 201 });
      }

      throw new Error(`Unhandled fetch: ${url}`);
    });
    vi.stubGlobal("fetch", fetchMock);

    render(
      <MemoryRouter>
        <RunPocPage />
      </MemoryRouter>,
    );

    const input = screen.getByLabelText("Input Files") as HTMLInputElement;
    fireEvent.change(input, {
      target: {
        files: [
          new File(["alpha"], "alpha.csv", { type: "text/csv" }),
          new File(["beta"], "beta.csv", { type: "text/csv" }),
        ],
      },
    });

    fireEvent.click(screen.getByRole("button", { name: "Start Run" }));

    await waitFor(() => {
      expect(
        fetchMock.mock.calls.filter(([candidate]) => {
          const url =
            typeof candidate === "string" ? candidate : candidate.toString();
          return url.startsWith("https://blob.example.com/");
        }),
      ).toHaveLength(2);
    });
    await waitFor(() => {
      expect(screen.getByText("bat_123")).toBeInTheDocument();
    });
    expect(screen.getByText("alpha.csv")).toBeInTheDocument();
    expect(screen.getByText("beta.csv")).toBeInTheDocument();
    expect(screen.getAllByText("100%")).toHaveLength(2);
    await waitFor(() => {
      expect(screen.getByText("run-1")).toBeInTheDocument();
      expect(screen.getByText("run-2")).toBeInTheDocument();
    });

    await waitFor(
      () => {
        expect(
          fetchMock.mock.calls.some(([candidate]) => {
            const url =
              typeof candidate === "string" ? candidate : candidate.toString();
            return url.endsWith("/runs/run-1");
          }),
        ).toBe(true);
        expect(
          fetchMock.mock.calls.some(([candidate]) => {
            const url =
              typeof candidate === "string" ? candidate : candidate.toString();
            return url.endsWith("/runs/run-2");
          }),
        ).toBe(true);
      },
      { timeout: 7_000 },
    );
    expect(window.sessionStorage.getItem("ade.run-poc")).toContain("bat_123");
  }, 12_000);

  it("uploads a single file directly to storage before creating the run", async () => {
    const fetchMock = vi.fn(
      async (input: RequestInfo | URL) => {
        const url = typeof input === "string" ? input : input.toString();
        if (url.endsWith("/uploads")) {
          return new Response(
            JSON.stringify({
              filePath:
                "workspaces/workspace-a/configs/config-v1/uploads/upl_123/alpha.csv",
              upload: {
                expiresAt: "2026-03-30T12:00:00Z",
                headers: { "content-type": "text/csv" },
                method: "PUT",
                url: "https://blob.example.com/alpha.csv?sas",
              },
            }),
            {
              headers: { "Content-Type": "application/json" },
              status: 200,
            },
          );
        }

        if (url.endsWith("/runs") && !url.includes("/downloads")) {
          return new Response(
            JSON.stringify({
              runId: "run-1",
              status: "pending",
            }),
            {
              headers: { "Content-Type": "application/json" },
              status: 202,
            },
          );
        }

        if (url.startsWith("https://blob.example.com/")) {
          return new Response(null, { status: 201 });
        }

        throw new Error(`Unhandled fetch: ${url}`);
      },
    );
    vi.stubGlobal("fetch", fetchMock);

    render(
      <MemoryRouter>
        <RunPocPage />
      </MemoryRouter>,
    );

    const input = screen.getByLabelText("Input Files") as HTMLInputElement;
    fireEvent.change(input, {
      target: {
        files: [new File(["alpha"], "alpha.csv", { type: "text/csv" })],
      },
    });

    fireEvent.click(screen.getByRole("button", { name: "Start Run" }));

    await waitFor(() => {
      expect(fetchMock).toHaveBeenCalledWith(
        "https://blob.example.com/alpha.csv?sas",
        expect.objectContaining({
          body: expect.any(File),
          method: "PUT",
        }),
      );
    });
    expect(fetchMock).toHaveBeenCalledWith(
      expect.stringContaining("/uploads"),
      expect.any(Object),
    );
    expect(fetchMock).toHaveBeenCalledWith(
      expect.stringContaining("/runs"),
      expect.any(Object),
    );
  });
});
