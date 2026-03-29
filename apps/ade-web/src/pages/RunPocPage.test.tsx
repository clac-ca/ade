import { fireEvent, render, screen, waitFor } from "@testing-library/react";
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
});
