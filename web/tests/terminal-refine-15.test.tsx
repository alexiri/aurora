import { act, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { TerminalTab, withCtrl } from "@/components/agents/terminal-tab";
import { wrap } from "./support/render";

// (P5-REFINE-15, computer-1, computer-12) The restart line from `boot_id`, the phone key row (Esc, Tab, Ctrl latch,
// arrows through the pane's own input path), the sentence above the pane, and no "Open in new window" without the Expert switch.

const { write, term } = vi.hoisted(() => ({ write: vi.fn(), term: { onData: null as null | ((d: string) => void) } }));
vi.mock("next/navigation", () => ({ useParams: () => ({ org: "acme", ws: "main", handle: "daneel" }) }));
vi.mock("@wterm/ghostty", () => ({ GhosttyCore: { load: vi.fn(() => Promise.resolve({ dispose: vi.fn() })) } }));
vi.mock("@wterm/react", () => ({
  useTerminal: () => ({ ref: { current: null }, write }),
  Terminal: ({ onData }: { onData: (d: string) => void }) => {
    term.onData = onData;
    return <div data-testid="wterm-pane" />;
  },
}));
const sandbox = vi.hoisted(() => ({ boot: "boot-1" }));
vi.mock("@/lib/api/conversations", () => ({ useSandbox: () => ({ data: { state: "running", boot_id: sandbox.boot } }) }));

class Socket {
  static OPEN = 1;
  static instances: Socket[] = [];
  readyState = 0;
  binaryType = "blob";
  sent: string[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((ev: unknown) => void) | null = null;
  onclose: ((ev: unknown) => void) | null = null;
  constructor() {
    Socket.instances.push(this);
  }
  send(data: Uint8Array | string) {
    if (typeof data !== "string") this.sent.push(new TextDecoder().decode(data));
  }
  close() {}
  open() {
    this.readyState = 1;
    this.onopen?.();
  }
}

beforeEach(() => {
  sandbox.boot = "boot-1";
  write.mockClear();
  vi.stubGlobal("WebSocket", Socket as unknown as typeof WebSocket);
  Socket.instances = [];
});
afterEach(() => vi.useRealTimers());

async function mount(more = true) {
  const view = wrap(<TerminalTab agentId="agent-1" frozen={false} />, { moreControls: more });
  await waitFor(() => expect(Socket.instances).toHaveLength(1));
  act(() => Socket.instances[0]!.open());
  return view;
}
const decoded = () => write.mock.calls.map(([b]) => new TextDecoder().decode(b as Uint8Array)).join("");

describe("the restart line (C-1058)", () => {
  it("prints one muted line when boot_id changes under the open session, and not on the first read", async () => {
    await mount();
    expect(decoded()).not.toContain("restarted");
    // The poll answers with a new boot id (the mock reads it on every render; the Ctrl latch is a cheap re-render).
    sandbox.boot = "boot-2";
    fireEvent.click(screen.getByRole("button", { name: "Ctrl" }));
    await waitFor(() => expect(decoded()).toMatch(/— The computer restarted at \d{1,2}:\d{2}.*; your running commands were stopped\. —/));
    fireEvent.click(screen.getByRole("button", { name: "Ctrl" }));
    expect(write.mock.calls).toHaveLength(1);
  });
});

describe("above the pane and the Expert pop-out", () => {
  it("replaces the old explanation by the keep-awake sentence", async () => {
    await mount();
    expect(screen.getByTestId("terminal-keep-awake").textContent).toBe("An open terminal keeps the computer awake, so a running command is never cut off.");
    expect(screen.queryByText(/A shell inside the agent/)).toBeNull();
  });
  it("offers Open in new window only with the Expert switch, and hides it on a phone (max-sm:hidden)", async () => {
    const view = await mount(true);
    const button = screen.getByRole("button", { name: "Open in new window" });
    expect(button.className).toContain("max-sm:hidden");
    view.unmount();
    Socket.instances = [];
    await mount(false);
    expect(screen.queryByRole("button", { name: "Open in new window" })).toBeNull();
  });
});

describe("phone key row (computer-12)", () => {
  it("sends Esc, Tab and the arrows through the pane's input path", async () => {
    await mount();
    const row = screen.getByRole("toolbar", { name: "Terminal keys" });
    expect(row.className).toContain("sm:hidden");
    for (const name of ["Esc", "Tab", "Up", "Down", "Left", "Right"]) fireEvent.click(screen.getByRole("button", { name }));
    expect(Socket.instances[0]!.sent).toEqual(["\x1b", "\t", "\x1b[A", "\x1b[B", "\x1b[D", "\x1b[C"]);
  });

  it("Ctrl is a latch: the next key from the keyboard goes out with Ctrl, then it releases", async () => {
    await mount();
    fireEvent.click(screen.getByRole("button", { name: "Ctrl" }));
    expect(screen.getByRole("button", { name: "Ctrl" })).toHaveAttribute("aria-pressed", "true");
    act(() => term.onData!("c"));
    expect(Socket.instances[0]!.sent).toEqual(["\x03"]);
    expect(screen.getByRole("button", { name: "Ctrl" })).toHaveAttribute("aria-pressed", "false");
    act(() => term.onData!("c"));
    expect(Socket.instances[0]!.sent).toEqual(["\x03", "c"]);
  });

  it("is not there until the terminal is connected", async () => {
    wrap(<TerminalTab agentId="agent-1" frozen={false} />);
    await waitFor(() => expect(Socket.instances).toHaveLength(1));
    expect(screen.queryByRole("toolbar", { name: "Terminal keys" })).toBeNull();
  });

  it("withCtrl maps a printable key to its control character and leaves the rest", () => {
    expect(withCtrl("a")).toBe("\x01");
    expect(withCtrl("[")).toBe("\x1b");
    expect(withCtrl("1")).toBe("1");
    expect(withCtrl("abc")).toBe("abc");
  });
});
