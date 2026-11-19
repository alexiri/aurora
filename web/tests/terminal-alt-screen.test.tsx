import { act, render, screen, waitFor } from "@testing-library/react";
import { useEffect, useRef } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { TerminalPane } from "@/components/agents/terminal-tab";

// Owner 2026-11-14: a full-screen app (claude, vim, less) is on the alternate screen, which has no scrollback. wterm still
// renders the shell's history above it, so the pane pins itself to the app's screen and stops scrolling, and the wheel
// goes to the app: forwarded by wterm when the app asked for mouse reports, otherwise sent as arrow keys.

const core = vi.hoisted(() => ({ alt: false, tracking: 0 as 0 | 1000, appKeys: false }));
// Stable like `useTerminal`'s own (a new function per render would re-run the pane's socket effect forever).
const { write } = vi.hoisted(() => ({ write: () => {} }));
const bridge = {
  usingAltScreen: () => core.alt,
  mouseTracking: () => core.tracking,
  cursorKeysApp: () => core.appKeys,
  getRows: () => 20,
};

vi.mock("next/navigation", () => ({ useParams: () => ({ org: "acme", ws: "main", handle: "daneel" }) }));
vi.mock("@wterm/ghostty", () => ({ GhosttyCore: { load: vi.fn(() => Promise.resolve({ dispose: vi.fn() })) } }));
vi.mock("@wterm/react", () => ({
  useTerminal: () => ({ ref: { current: null }, write }),
  Terminal: ({ onReady, "data-alt-screen": alt }: { onReady: (wt: unknown) => void; "data-alt-screen"?: string }) => {
    const el = useRef<HTMLDivElement>(null);
    useEffect(() => {
      // jsdom has no layout: a pane of 400 px showing a 2000 px tall history + screen.
      Object.defineProperty(el.current!, "scrollHeight", { configurable: true, value: 2000 });
      Object.defineProperty(el.current!, "clientHeight", { configurable: true, value: 400 });
      Object.defineProperty(el.current!, "scrollTop", { configurable: true, writable: true, value: 0 });
      onReady({ element: el.current, bridge });
      // eslint-disable-next-line react-hooks/exhaustive-deps
    }, []);
    return <div ref={el} data-testid="pane" data-alt-screen={alt} />;
  },
}));
vi.mock("@/lib/api/conversations", () => ({ useSandbox: () => ({ data: { state: "running" } }) }));

class Socket {
  static OPEN = 1;
  static last: Socket | null = null;
  readyState = Socket.OPEN;
  binaryType = "blob";
  sent: unknown[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onclose: ((ev: unknown) => void) | null = null;
  constructor() {
    Socket.last = this;
    queueMicrotask(() => this.onopen?.());
  }
  send(d: unknown) {
    this.sent.push(d);
  }
  close() {}
}

const decoded = (s: Socket) =>
  // `ArrayBuffer.isView`, not `instanceof Uint8Array`: TextEncoder's bytes come from another realm under jsdom.
  s.sent.filter((d): d is Uint8Array => ArrayBuffer.isView(d)).map((d) => new TextDecoder().decode(d));

/** Output arrives; the pane re-reads the screen mode after wterm has rendered (two frames). */
async function output() {
  await act(async () => {
    Socket.last!.onmessage?.({ data: new ArrayBuffer(1) });
    await new Promise((r) => setTimeout(r, 50));
  });
}

function wheel(target: Element, deltaY: number) {
  const e = new WheelEvent("wheel", { deltaY, deltaMode: 1, bubbles: true, cancelable: true });
  target.dispatchEvent(e);
  return e;
}

beforeEach(() => {
  Object.assign(core, { alt: false, tracking: 0, appKeys: false });
  vi.stubGlobal("WebSocket", Socket as unknown as typeof WebSocket);
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => setTimeout(() => cb(0), 0));
});

async function mount() {
  render(<TerminalPane agentId="agent-1" frozen={false} />);
  const pane = await screen.findByTestId("pane");
  await waitFor(() => expect(Socket.last).not.toBeNull());
  return pane;
}

describe("terminal pane on the alternate screen", () => {
  it("marks the pane and pins it to the app's screen while a full-screen app runs", async () => {
    const pane = await mount();
    await output();
    expect(pane).not.toHaveAttribute("data-alt-screen");
    core.alt = true;
    await output();
    expect(pane).toHaveAttribute("data-alt-screen");
    // A scroll up (keyboard, scrollbar drag) snaps back to the app's screen.
    pane.scrollTop = 100;
    await act(async () => {
      pane.dispatchEvent(new Event("scroll"));
    });
    expect(pane.scrollTop).toBe(2000);
    expect(screen.queryByRole("button", { name: "Jump to latest" })).not.toBeInTheDocument();
    core.alt = false;
    await output();
    expect(pane).not.toHaveAttribute("data-alt-screen");
  });

  it("sends the wheel as arrow keys when the app did not ask for the mouse", async () => {
    const pane = await mount();
    core.alt = true;
    await output();
    const up = wheel(pane, -3);
    expect(up.defaultPrevented).toBe(true);
    wheel(pane, 2);
    core.appKeys = true;
    wheel(pane, -1);
    expect(decoded(Socket.last!)).toEqual(["\x1b[A\x1b[A\x1b[A", "\x1b[B\x1b[B", "\x1bOA"]);
  });

  it("leaves the wheel to wterm when the app reports the mouse, with the pane on the app's screen", async () => {
    const pane = await mount();
    core.alt = true;
    core.tracking = 1000;
    await output();
    pane.scrollTop = 0;
    const e = wheel(pane, -3);
    expect(e.defaultPrevented).toBe(false);
    expect(pane.scrollTop).toBe(2000);
    expect(decoded(Socket.last!)).toEqual([]);
  });

  it("does not touch the wheel on the normal screen (history scrolls)", async () => {
    const pane = await mount();
    await output();
    const e = wheel(pane, -3);
    expect(e.defaultPrevented).toBe(false);
    expect(decoded(Socket.last!)).toEqual([]);
  });
});
