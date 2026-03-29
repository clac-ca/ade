import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TerminalPocPage } from "./TerminalPocPage";

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

describe("TerminalPocPage", () => {
  beforeEach(() => {
    MockWebSocket.instances = [];
    vi.stubGlobal("WebSocket", MockWebSocket);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("ignores stale socket events after reconnecting", async () => {
    render(
      <MemoryRouter>
        <TerminalPocPage />
      </MemoryRouter>,
    );

    fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    const firstSocket = MockWebSocket.instances[0]!;
    expect(firstSocket).toBeDefined();

    fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    const secondSocket = MockWebSocket.instances[1]!;
    expect(secondSocket).toBeDefined();

    expect(
      screen.getByText("Connecting to the terminal bridge..."),
    ).toBeInTheDocument();

    firstSocket.emit("error");
    firstSocket.emit("close");

    expect(
      screen.getByText("Connecting to the terminal bridge..."),
    ).toBeInTheDocument();

    secondSocket.emit("message", { data: JSON.stringify({ type: "ready" }) });

    await waitFor(() => {
      expect(screen.getByText("Connected.")).toBeInTheDocument();
    });
  });
});
