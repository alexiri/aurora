import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { TerminalTab } from "@/components/agents/terminal-tab";
import { RecordingSocket } from "./support/terminal";

// Copy and paste in the terminal pane (FIX-156, C-622). wterm's own key handler reads `e.key === "v"` for paste, so
// Ctrl+Shift+V reached the shell as ^V; Ctrl+Shift+C without a selection was an interrupt; the OSC 52 confirmation
// threw where `navigator.clipboard` is missing. The pane is a stub here (no WASM in jsdom): its textarea plays the part
// of wterm's input — it records the keydowns that reach it and turns a `paste` event into `onData`, as wterm's
// `handlePaste` does — so what is under test is the wrapper's key/paste/menu handling and the clipboard fallbacks.

type TerminalProps = {
  onData: (data: string) => void;
  onReady: (wt: unknown) => void;
  onClipboardWrite: (text: string) => void;
};

let paneProps: TerminalProps | null = null;
/** Keydowns that reached the stub's textarea (what wterm would have encoded). */
let reached: string[] = [];

const { write } = vi.hoisted(() => ({ write: vi.fn() }));

vi.mock("next/navigation", () => ({ useParams: () => ({ org: "acme", ws: "main", handle: "daneel" }) }));
vi.mock("@wterm/ghostty", () => ({ GhosttyCore: { load: vi.fn(() => Promise.resolve({ dispose: vi.fn() })) } }));
vi.mock("@wterm/react", async () => {
  const React = await import("react");
  return {
    useTerminal: () => ({ ref: { current: null }, write }),
    Terminal: (props: unknown) => {
      const p = props as TerminalProps;
      paneProps = p;
      const host = React.useRef<HTMLDivElement>(null);
      React.useEffect(() => {
        p.onReady({ element: host.current, bridge: { usingAltScreen: () => false, mouseTracking: () => 0 }, selectAll: () => Promise.resolve(true) });
      }, []); // eslint-disable-line react-hooks/exhaustive-deps
      return (
        <div ref={host} data-testid="wterm-pane">
          <textarea
            data-testid="wterm-input"
            onKeyDown={(e) => reached.push(`${e.ctrlKey ? "C-" : ""}${e.shiftKey ? "S-" : ""}${e.key}`)}
            onPaste={(e) => {
              const text = (e.nativeEvent as unknown as { clipboardData?: { getData(t: string): string } }).clipboardData?.getData("text");
              if (text) p.onData(text);
            }}
          />
        </div>
      );
    },
  };
});
vi.mock("@/lib/api/conversations", () => ({ useSandbox: () => ({ data: { state: "running" } }) }));

/** jsdom has neither `DataTransfer` nor `ClipboardEvent`: the two fields `pasteInto` and the stub use. */
class FakeDataTransfer {
  private d = new Map<string, string>();
  setData(t: string, v: string) {
    this.d.set(t, v);
  }
  getData(t: string) {
    return this.d.get(t === "text" ? "text/plain" : t) ?? "";
  }
}
class FakeClipboardEvent extends Event {
  clipboardData: FakeDataTransfer | null;
  constructor(type: string, init: EventInit & { clipboardData?: FakeDataTransfer } = {}) {
    super(type, init);
    this.clipboardData = init.clipboardData ?? null;
  }
}

const PASTE_WAIT = 400; // longer than PASTE_FALLBACK_MS

const sleep = (ms: number) => act(async () => void (await new Promise((r) => setTimeout(r, ms))));
const text = (ws: RecordingSocket) => ws.sent.map((f) => (typeof f === "string" ? f : new TextDecoder().decode(f as Uint8Array))).join("");

async function mount() {
  render(<TerminalTab agentId="agent-1" frozen={false} />);
  await waitFor(() => expect(RecordingSocket.instances).toHaveLength(1));
  await screen.findByTestId("wterm-input");
  return { ws: RecordingSocket.instances[0], input: screen.getByTestId("wterm-input") };
}

function clipboard(read: (() => Promise<string>) | undefined, write?: (t: string) => Promise<void>) {
  Object.defineProperty(navigator, "clipboard", {
    value: read || write ? { readText: read, writeText: write } : undefined,
    configurable: true,
  });
}

beforeEach(() => {
  vi.stubGlobal("WebSocket", RecordingSocket as unknown as typeof WebSocket);
  vi.stubGlobal("DataTransfer", FakeDataTransfer);
  vi.stubGlobal("ClipboardEvent", FakeClipboardEvent);
  RecordingSocket.instances = [];
  paneProps = null;
  reached = [];
});

afterEach(() => {
  vi.restoreAllMocks();
  clipboard(undefined);
});

describe("Ctrl+Shift+V", () => {
  it("never reaches wterm (no ^V) and pastes the clipboard through its paste path when the browser raised no paste", async () => {
    clipboard(() => Promise.resolve("echo hi"));
    const { ws, input } = await mount();
    fireEvent.keyDown(input, { key: "V", ctrlKey: true, shiftKey: true });
    expect(reached, "the chord must not reach wterm's key handler").toEqual([]);
    await sleep(PASTE_WAIT);
    expect(text(ws)).toBe("echo hi");
  });

  it("does not read the clipboard when the browser's own paste event arrives", async () => {
    const read = vi.fn(() => Promise.resolve("from the clipboard"));
    clipboard(read);
    const { ws, input } = await mount();
    fireEvent.keyDown(input, { key: "V", ctrlKey: true, shiftKey: true });
    const data = new FakeDataTransfer();
    data.setData("text/plain", "native");
    fireEvent(input, new FakeClipboardEvent("paste", { bubbles: true, cancelable: true, clipboardData: data }));
    await sleep(PASTE_WAIT);
    expect(read).not.toHaveBeenCalled();
    expect(text(ws)).toBe("native");
  });

  it("says so when the clipboard cannot be read (insecure context) and sends nothing", async () => {
    clipboard(undefined);
    const { ws, input } = await mount();
    fireEvent.keyDown(input, { key: "V", ctrlKey: true, shiftKey: true });
    await sleep(PASTE_WAIT);
    expect(screen.getByText(/Could not read the clipboard/)).toBeInTheDocument();
    expect(ws.sent).toHaveLength(0);
  });

  it("leaves Meta+V and plain Ctrl+V to wterm and the browser", async () => {
    const read = vi.fn(() => Promise.resolve("x"));
    clipboard(read);
    const { input } = await mount();
    fireEvent.keyDown(input, { key: "v", ctrlKey: true });
    fireEvent.keyDown(input, { key: "v", metaKey: true });
    await sleep(PASTE_WAIT);
    expect(reached).toEqual(["C-v", "v"]);
    expect(read).not.toHaveBeenCalled();
  });

  it("while disconnected sends nothing and keeps the not-sent notice", async () => {
    clipboard(() => Promise.resolve("secret"));
    const { ws, input } = await mount();
    act(() => ws.onclose?.({ code: 4403, wasClean: true }));
    ws.readyState = 3;
    fireEvent.keyDown(input, { key: "V", ctrlKey: true, shiftKey: true });
    await sleep(PASTE_WAIT);
    expect(ws.sent).toHaveLength(0);
    expect(screen.getByTestId("terminal-not-connected")).toHaveTextContent("what you typed was not sent");
  });
});

describe("Ctrl+Shift+C", () => {
  it("with no selection is swallowed: no ^C reaches wterm", async () => {
    const { input } = await mount();
    const notPrevented = fireEvent.keyDown(input, { key: "C", ctrlKey: true, shiftKey: true });
    expect(notPrevented, "default prevented").toBe(false);
    expect(reached).toEqual([]);
  });

  it("with a selection reaches wterm, which copies it", async () => {
    vi.spyOn(document, "getSelection").mockReturnValue({ toString: () => "picked" } as Selection);
    const { input } = await mount();
    fireEvent.keyDown(input, { key: "C", ctrlKey: true, shiftKey: true });
    expect(reached).toEqual(["C-S-C"]);
  });

  it("plain Ctrl+C is still the interrupt", async () => {
    const { input } = await mount();
    fireEvent.keyDown(input, { key: "c", ctrlKey: true });
    expect(reached).toEqual(["C-c"]);
  });
});

describe("OSC 52 confirmation", () => {
  it("copies through the legacy path when navigator.clipboard is missing, then clears", async () => {
    clipboard(undefined);
    const exec = vi.fn(() => true);
    Object.defineProperty(document, "execCommand", { value: exec, configurable: true });
    await mount();
    act(() => paneProps!.onClipboardWrite("from the app"));
    fireEvent.click(screen.getByRole("button", { name: "Copy to clipboard" }));
    await waitFor(() => expect(screen.queryByRole("button", { name: "Copy to clipboard" })).toBeNull());
    expect(exec).toHaveBeenCalledWith("copy");
  });

  it("keeps the text and says how to copy it when nothing works, and never throws", async () => {
    clipboard(undefined);
    Object.defineProperty(document, "execCommand", { value: () => false, configurable: true });
    await mount();
    act(() => paneProps!.onClipboardWrite("from the app"));
    fireEvent.click(screen.getByRole("button", { name: "Copy to clipboard" }));
    expect(await screen.findByText(/Could not copy — select the text and press Ctrl\+Shift\+C/)).toBeInTheDocument();
    expect(screen.getByText("from the app")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Copy to clipboard" })).toBeInTheDocument();
  });

  it("uses the async Clipboard API when there is one", async () => {
    const writeText = vi.fn(() => Promise.resolve());
    clipboard(undefined, writeText);
    await mount();
    act(() => paneProps!.onClipboardWrite("from the app"));
    fireEvent.click(screen.getByRole("button", { name: "Copy to clipboard" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("from the app"));
    await waitFor(() => expect(screen.queryByRole("button", { name: "Copy to clipboard" })).toBeNull());
  });
});

describe("sandbox notice (FIX-155)", () => {
  it("always says the terminal runs in the agent's sandbox, who can change it and who can see the input", async () => {
    await mount();
    expect(screen.getByTestId("terminal-sandbox-notice")).toHaveTextContent(
      "This terminal runs inside the agent's sandbox: the agent can change what runs here and can see what you type.",
    );
    // Not a dialog and not dismissible.
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.getByTestId("terminal-sandbox-notice").querySelector("button")).toBeNull();
  });
});

describe("help text", () => {
  it("names the platform's shortcuts in the pane footer", async () => {
    await mount();
    expect(screen.getByText(/Copy: select, then Ctrl\+Shift\+C\. Paste: Ctrl\+Shift\+V\./)).toBeInTheDocument();
  });
});
