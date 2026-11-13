import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
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
const sandboxState = vi.hoisted(() => ({ state: "running" }));
vi.mock("@/lib/api/conversations", () => ({ useSandbox: () => ({ data: { state: sandboxState.state } }) }));

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
  fail(code: number, wasClean = false) {
    this.onclose?.({ code, wasClean });
  }
}

beforeEach(() => {
  sandboxState.state = "running";
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
  await waitFor(() => expect(Socket.instances).toHaveLength(1));
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

  // FIX-098: the code is classified first, even while the sandbox is waking.
  it.each([4403, 4404, 4409])("close %i while the sandbox is starting is final: notice, no further socket", async (code) => {
    sandboxState.state = "starting";
    await mount();
    act(() => Socket.instances[0].fail(code));
    expect(screen.getByRole("alert")).toBeInTheDocument();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(60000);
    });
    expect(Socket.instances).toHaveLength(1);
  });

  // The API sends 4502/4503 as real close frames: wasClean is true.
  it.each([4502, 4503])("a clean close %i reconnects, visibly, up to the cap", async (code) => {
    await mount();
    for (let attempt = 1; attempt <= 4; attempt += 1) {
      act(() => Socket.instances.at(-1)!.fail(code, true));
      expect(screen.getByText("Reconnecting…")).toBeInTheDocument();
      expect(screen.getByText(`(attempt ${attempt} of 4)`, { exact: false })).toBeInTheDocument();
      await act(async () => {
        await vi.advanceTimersByTimeAsync(8000);
      });
      expect(Socket.instances).toHaveLength(attempt + 1);
    }
    act(() => Socket.instances.at(-1)!.fail(code, true));
    expect(screen.getByRole("alert")).toBeInTheDocument();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(60000);
    });
    expect(Socket.instances).toHaveLength(5);
  });

  it("waking retries stop at the cap (a sandbox stuck starting is not a handshake every 8 s forever)", async () => {
    sandboxState.state = "starting";
    await mount();
    for (let attempt = 1; attempt <= 12; attempt += 1) {
      act(() => Socket.instances.at(-1)!.fail(1006));
      await act(async () => {
        await vi.advanceTimersByTimeAsync(8000);
      });
    }
    expect(Socket.instances).toHaveLength(13);
    act(() => Socket.instances.at(-1)!.fail(1006));
    expect(screen.getByRole("alert")).toHaveTextContent("still starting");
    await act(async () => {
      await vi.advanceTimersByTimeAsync(60000);
    });
    expect(Socket.instances).toHaveLength(13);
  });

  it("shows that the pane is not connected while reconnecting", async () => {
    await mount();
    act(() => Socket.instances[0].fail(1006));
    expect(screen.getByTestId("terminal-not-connected")).toHaveTextContent("Not connected");
  });
});
