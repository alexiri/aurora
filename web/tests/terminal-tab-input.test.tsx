import { render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { TerminalTab } from "@/components/agents/terminal-tab";

// The terminal input seam (FIX-054): wterm hands `onData` a *string*; the tab must put it
// on the wire as UTF-8 **Binary** frames — chunked to `TERMINAL_CHUNK` (16 KiB; the runner
// closes the leg on a larger frame) — because the runner's terminal leg reads only Binary
// for keystrokes (`C-619`: Text frames are never parsed as bytes). `pty.resize` stays the
// one Text control. These tests mount the real tab around a mocked wterm pane (no WASM in
// jsdom) and a stub WebSocket, then drive the captured props exactly as wterm would.

type TerminalProps = {
  onData: (data: string) => void;
  onResize: (cols: number, rows: number) => void;
};

// The props the tab passes to the wterm pane, captured by the mock below.
let paneProps: TerminalProps | null = null;

// `useTerminal`'s `write` must be stable across renders (the real one is a `useCallback`
// with an empty dep list): the socket effect depends on it, and a fresh fn per render
// would re-run the effect — and re-open the socket — on every render, forever.
const { write } = vi.hoisted(() => ({ write: vi.fn() }));

vi.mock("@wterm/ghostty", () => ({
  GhosttyCore: {
    load: vi.fn(() =>
      Promise.resolve({
        dispose: vi.fn(),
      }),
    ),
  },
}));
vi.mock("@wterm/react", () => ({
  // No WASM or layout in jsdom: the pane is a stub; the tab's props are the seam under test.
  useTerminal: () => ({ ref: { current: null }, write }),
  Terminal: (props: unknown) => {
    paneProps = props as TerminalProps;
    return <div data-testid="wterm-pane" />;
  },
}));
vi.mock("@/lib/api/conversations", () => ({
  useSandbox: () => ({ data: { state: "running" } }),
}));

/** A WebSocket stub that records everything the tab sends, in order. */
class RecordingSocket {
  static OPEN = 1;
  static instances: RecordingSocket[] = [];
  readyState = RecordingSocket.OPEN;
  binaryType = "blob";
  sent: Array<Uint8Array | string> = [];
  onopen: (() => void) | null = null;
  onmessage: ((ev: unknown) => void) | null = null;
  onclose: ((ev: unknown) => void) | null = null;
  constructor(url: string) {
    expect(url).toBe("/v1/agents/agent-1/terminal");
    RecordingSocket.instances.push(this);
    // The tab assigns its handlers synchronously after the constructor; fire open then.
    queueMicrotask(() => this.onopen?.());
  }
  send(data: Uint8Array | string) {
    this.sent.push(data);
  }
  close() {}
}

const encoder = new TextEncoder();

beforeEach(() => {
  vi.stubGlobal("WebSocket", RecordingSocket as unknown as typeof WebSocket);
  RecordingSocket.instances = [];
  paneProps = null;
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

/** True when `d` is a typed-array view (Binary frame), realm-agnostic (jsdom vs Node). */
function isBinary(d: Uint8Array | string): d is Uint8Array {
  return typeof d === "object" && ArrayBuffer.isView(d);
}

/** The socket the tab opened (the constructor fires onopen on a microtask). */
function openSocket(): RecordingSocket {
  return RecordingSocket.instances[0];
}

describe("terminal tab input framing (FIX-054)", () => {
  it("sends keystrokes as Binary frames with the exact UTF-8 bytes", async () => {
    render(<TerminalTab agentId="agent-1" frozen={false} />);
    await vi.waitFor(() => expect(paneProps).not.toBeNull());
    const ws = openSocket();

    paneProps!.onData("ls\n");
    expect(ws.sent).toHaveLength(1);
    const frame = ws.sent[0];
    expect(isBinary(frame)).toBe(true);
    expect(Array.from(frame as Uint8Array)).toEqual(Array.from(encoder.encode("ls\n")));
  });

  it("encodes multi-byte UTF-8 exactly", async () => {
    render(<TerminalTab agentId="agent-1" frozen={false} />);
    await vi.waitFor(() => expect(paneProps).not.toBeNull());
    const ws = openSocket();

    paneProps!.onData("héllo ☃\r");
    expect(ws.sent).toEqual([encoder.encode("héllo ☃\r")]);
  });

  it("chunks a >16 KiB paste into ordered Binary frames of at most 16384 bytes", async () => {
    render(<TerminalTab agentId="agent-1" frozen={false} />);
    await vi.waitFor(() => expect(paneProps).not.toBeNull());
    const ws = openSocket();

    // A 40 KiB paste (the clipboard arrives as one onData string): one ASCII half, one
    // multi-byte tail, so the chunk boundaries never split a code point's bytes silently.
    const paste = "a".repeat(20 * 1024) + "☃".repeat(20 * 1024 / 3);
    paneProps!.onData(paste);

    const frames = ws.sent.filter(isBinary);
    expect(ws.sent.length, "every paste frame is Binary").toBe(frames.length);
    expect(frames.length).toBeGreaterThan(2);
    for (const f of frames) {
      expect(f.byteLength).toBeLessThanOrEqual(16384);
      expect(f.byteLength).toBeGreaterThan(0);
    }
    const joined = new Uint8Array(frames.reduce((n, f) => n + f.byteLength, 0));
    let at = 0;
    for (const f of frames) {
      joined.set(f, at);
      at += f.byteLength;
    }
    expect(Array.from(joined)).toEqual(Array.from(encoder.encode(paste)));
  });

  it("keeps pty.resize on Text as the JSON control with pty_id: 0", async () => {
    render(<TerminalTab agentId="agent-1" frozen={false} />);
    await vi.waitFor(() => expect(paneProps).not.toBeNull());
    const ws = openSocket();

    paneProps!.onResize(120, 40);
    expect(ws.sent).toHaveLength(1);
    const frame = ws.sent[0];
    expect(typeof frame).toBe("string");
    const v = JSON.parse(frame as string);
    expect(v).toEqual({ kind: "pty.resize", pty_id: 0, cols: 120, rows: 40 });
  });
});
