import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { TerminalTab } from "@/components/agents/terminal-tab";

// FIX-067: an abnormal close (1006: a dropped connection or a failed handshake) retries with the
// documented backoff, a few times, visibly; the terminal close codes (4403/4404/4409) end the pane.

const { write } = vi.hoisted(() => ({ write: vi.fn() }));
vi.mock("@wterm/ghostty", () => ({ GhosttyCore: { load: vi.fn(() => Promise.resolve({ dispose: vi.fn() })) } }));
vi.mock("@wterm/react", () => ({
  useTerminal: () => ({ ref: { current: null }, write }),
  Terminal: () => <div data-testid="wterm-pane" />,
}));
vi.mock("@/lib/api/conversations", () => ({ useSandbox: () => ({ data: { state: "running" } }) }));

class Socket {
  static OPEN = 1;
  static instances: Socket[] = [];
  readyState = 0;
  binaryType = "blob";
  onopen: (() => void) | null = null;
  onmessage: ((ev: unknown) => void) | null = null;
  onclose: ((ev: unknown) => void) | null = null;
  constructor() {
    Socket.instances.push(this);
  }
  send() {}
  close() {}
  fail(code: number) {
    this.onclose?.({ code, wasClean: false });
  }
}

beforeEach(() => {
  vi.stubGlobal("WebSocket", Socket as unknown as typeof WebSocket);
  Socket.instances = [];
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

async function mount() {
  render(<TerminalTab agentId="agent-1" frozen={false} />);
  await vi.waitFor(() => expect(Socket.instances).toHaveLength(1));
  // Only timers are faked, so the core load's promise chain keeps running.
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
}

describe("terminal reconnect", () => {
  it("reconnects after a 1006 with backoff, then ends after four failed attempts", async () => {
    await mount();
    for (let attempt = 1; attempt <= 4; attempt += 1) {
      act(() => Socket.instances.at(-1)!.fail(1006));
      expect(screen.getByText("Reconnecting…")).toBeInTheDocument();
      const before = Socket.instances.length;
      await act(async () => {
        await vi.advanceTimersByTimeAsync(8000);
      });
      expect(Socket.instances.length).toBe(before + 1);
    }
    act(() => Socket.instances.at(-1)!.fail(1006));
    expect(screen.getByRole("alert")).toHaveTextContent("ended unexpectedly");
    await act(async () => {
      await vi.advanceTimersByTimeAsync(20000);
    });
    expect(Socket.instances).toHaveLength(5);
  });

  it.each([4403, 4404, 4409])("close %i stays final: no reconnect", async (code) => {
    await mount();
    act(() => Socket.instances[0].fail(code));
    expect(screen.getByRole("alert")).toBeInTheDocument();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(20000);
    });
    expect(Socket.instances).toHaveLength(1);
  });
});
