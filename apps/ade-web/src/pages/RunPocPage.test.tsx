import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { RunPocPage } from "./RunPocPage";

class MockWebSocket {
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSING = 2;
  static CLOSED = 3;
  static instances: MockWebSocket[] = [];

  readonly listeners = new Map<string, Set<(event?: EventLike) => void>>();
  readyState = MockWebSocket.OPEN;
  sent: string[] = [];
  url: string;

  constructor(url: string) {
    this.url = url;
    MockWebSocket.instances.push(this);
  }

  addEventListener(type: string, listener: (event?: EventLike) => void) {
    const listeners = this.listeners.get(type) ?? new Set();
    listeners.add(listener);
    this.listeners.set(type, listeners);
  }

  close() {
    this.readyState = MockWebSocket.CLOSED;
    this.emit("close");
  }

  emit(type: string, event?: EventLike) {
    for (const listener of this.listeners.get(type) ?? []) {
      listener(event);
    }
  }

  send(data: string) {
    this.sent.push(data);
  }
}

type EventLike = {
  data?: string;
};

describe("RunPocPage", () => {
  beforeEach(() => {
    MockWebSocket.instances = [];
    window.sessionStorage.clear();
    vi.stubGlobal("WebSocket", MockWebSocket);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("ignores stale socket events after reconnecting", async () => {
    render(
      <MemoryRouter>
        <RunPocPage />
      </MemoryRouter>,
    );

    fireEvent.change(screen.getByLabelText("Run ID"), {
      target: { value: "run-1" },
    });

    fireEvent.click(screen.getByRole("button", { name: "Resume" }));
    const firstSocket = MockWebSocket.instances[0];
    expect(firstSocket).toBeDefined();
    if (!firstSocket) {
      throw new Error("missing first socket");
    }

    fireEvent.click(screen.getByRole("button", { name: "Resume" }));
    const secondSocket = MockWebSocket.instances[1];
    expect(secondSocket).toBeDefined();
    if (!secondSocket) {
      throw new Error("missing second socket");
    }

    expect(
      screen.getByText("Connecting to the run events stream..."),
    ).toBeInTheDocument();

    firstSocket.emit("close");

    expect(
      screen.getByText("Connecting to the run events stream..."),
    ).toBeInTheDocument();

    secondSocket.emit("open");
    secondSocket.emit("message", {
      data: JSON.stringify({
        type: "hello",
        lastSeq: 0,
        runId: "run-1",
        status: "running",
      }),
    });
    secondSocket.emit("message", {
      data: JSON.stringify({
        type: "status",
        seq: 1,
        phase: "installPackages",
        state: "started",
      }),
    });

    await waitFor(() => {
      expect(
        screen.getByText("Run phase: installPackages."),
      ).toBeInTheDocument();
    });
  });
});
