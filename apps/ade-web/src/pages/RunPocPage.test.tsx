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

function requestFrom(input: RequestInfo | URL, init?: RequestInit): Request {
  return input instanceof Request && init === undefined
    ? input
    : new Request(input, init);
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
        phase: "install",
        runId: "run-1",
        state: "started",
      }),
      lastEventId: "1",
    });

    expect(screen.queryByText("Run phase: install.")).not.toBeInTheDocument();

    secondSource.emit("run.status", {
      data: JSON.stringify({
        phase: "execute",
        runId: "run-1",
        state: "started",
      }),
      lastEventId: "2",
    });

    await waitFor(() => {
      expect(screen.getByText("Run phase: execute.")).toBeInTheDocument();
    });
  });

  it("uploads a bulk batch directly to storage and polls run status", async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = requestFrom(input).url;
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
          const candidateUrl = requestFrom(candidate).url;
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

    const input = screen.getByLabelText("Input Files");
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
          const url = requestFrom(candidate).url;
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
            const url = requestFrom(candidate).url;
            return url.endsWith("/runs/run-1");
          }),
        ).toBe(true);
        expect(
          fetchMock.mock.calls.some(([candidate]) => {
            const url = requestFrom(candidate).url;
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
      async (...args: [RequestInfo | URL, RequestInit?]) => {
        const [input] = args;
        const url = requestFrom(input).url;
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

    const input = screen.getByLabelText("Input Files");
    fireEvent.change(input, {
      target: {
        files: [new File(["alpha"], "alpha.csv", { type: "text/csv" })],
      },
    });

    fireEvent.click(screen.getByRole("button", { name: "Start Run" }));

    await waitFor(() => {
      expect(
        fetchMock.mock.calls.some(
          ([candidate, init]) =>
            requestFrom(candidate, init).url ===
              "https://blob.example.com/alpha.csv?sas" &&
            requestFrom(candidate, init).method === "PUT",
        ),
      ).toBe(true);
    });
    expect(
      fetchMock.mock.calls.some(([candidate]) =>
        requestFrom(candidate).url.endsWith("/uploads"),
      ),
    ).toBe(true);
    expect(
      fetchMock.mock.calls.some(([candidate]) =>
        requestFrom(candidate).url.endsWith("/runs"),
      ),
    ).toBe(true);

    const uploadIndex = fetchMock.mock.calls.findIndex(
      ([candidate]) =>
        requestFrom(candidate).url === "https://blob.example.com/alpha.csv?sas",
    );
    const createRunIndex = fetchMock.mock.calls.findIndex(([candidate]) =>
      requestFrom(candidate).url.endsWith("/runs"),
    );
    expect(uploadIndex).toBeGreaterThanOrEqual(0);
    expect(createRunIndex).toBeGreaterThan(uploadIndex);
  });

  it("loads final run details instead of opening SSE for an already completed run", async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = requestFrom(input).url;
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
            status: "succeeded",
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
            errorMessage: null,
            inputPath:
              "workspaces/workspace-a/configs/config-v1/uploads/upl_123/alpha.csv",
            logPath:
              "workspaces/workspace-a/configs/config-v1/runs/run-1/logs/events.ndjson",
            outputPath:
              "workspaces/workspace-a/configs/config-v1/runs/run-1/output/normalized.xlsx",
            phase: null,
            runId: "run-1",
            status: "succeeded",
            validationIssues: [],
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

    const input = screen.getByLabelText("Input Files");
    fireEvent.change(input, {
      target: {
        files: [new File(["alpha"], "alpha.csv", { type: "text/csv" })],
      },
    });

    fireEvent.click(screen.getByRole("button", { name: "Start Run" }));

    await waitFor(() => {
      expect(screen.getByText("Run succeeded.")).toBeInTheDocument();
    });
    expect(screen.getByText("completed")).toBeInTheDocument();
    expect(MockEventSource.instances).toHaveLength(0);
  });

  it("surfaces network failures from the API client during upload negotiation", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        throw new Error("Network down");
      }),
    );

    render(
      <MemoryRouter>
        <RunPocPage />
      </MemoryRouter>,
    );

    const input = screen.getByLabelText("Input Files");
    fireEvent.change(input, {
      target: {
        files: [new File(["alpha"], "alpha.csv", { type: "text/csv" })],
      },
    });

    fireEvent.click(screen.getByRole("button", { name: "Start Run" }));

    await waitFor(() => {
      expect(screen.getByText("Network down")).toBeInTheDocument();
    });
  });
});
