import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { TerminalTab } from "@/components/agents/terminal-tab";

// FIX-138 (D-246): the Shell selector. bash is the server's default, so the default socket URL
// carries no query; zsh names itself with `?shell=zsh`; the choice is remembered per browser
// and agent and applies to the next session only (the open socket is not re-opened).

const { write } = vi.hoisted(() => ({ write: vi.fn() }));
vi.mock("next/navigation", () => ({ useParams: () => ({ org: "acme", ws: "main", handle: "daneel" }) }));
vi.mock("@wterm/ghostty", () => ({ GhosttyCore: { load: vi.fn(() => Promise.resolve({ dispose: vi.fn() })) } }));
vi.mock("@wterm/react", () => ({
  useTerminal: () => ({ ref: { current: null }, write }),
  Terminal: () => <div data-testid="wterm-pane" />,
}));
vi.mock("@/lib/api/conversations", () => ({ useSandbox: () => ({ data: { state: "running" } }) }));

class UrlSocket {
  static OPEN = 1;
  static urls: string[] = [];
  readyState = UrlSocket.OPEN;
  binaryType = "blob";
  onopen: (() => void) | null = null;
  onmessage: ((ev: unknown) => void) | null = null;
  onclose: ((ev: unknown) => void) | null = null;
  constructor(url: string) {
    UrlSocket.urls.push(url);
    queueMicrotask(() => this.onopen?.());
  }
  send() {}
  close() {}
}

beforeEach(() => {
  vi.stubGlobal("WebSocket", UrlSocket as unknown as typeof WebSocket);
  UrlSocket.urls = [];
  window.localStorage.clear();
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("terminal shell choice (FIX-138)", () => {
  it("connects without a query for bash", async () => {
    render(<TerminalTab agentId="agent-1" frozen={false} />);
    await waitFor(() => expect(UrlSocket.urls).toHaveLength(1));
    expect(UrlSocket.urls[0]).toBe("/v1/agents/agent-1/terminal");
    expect(screen.getByRole("radio", { name: "bash" })).toBeChecked();
  });

  it("connects with ?shell=zsh when zsh was chosen earlier in this browser", async () => {
    window.localStorage.setItem("aurora.terminal.shell.agent-1", "zsh");
    render(<TerminalTab agentId="agent-1" frozen={false} />);
    await waitFor(() => expect(UrlSocket.urls.length).toBeGreaterThan(0));
    expect(screen.getByRole("radio", { name: "zsh" })).toBeChecked();
    expect(UrlSocket.urls.at(-1)).toBe("/v1/agents/agent-1/terminal?shell=zsh");
  });

  it("remembers a new choice and leaves the open session alone", async () => {
    render(<TerminalTab agentId="agent-1" frozen={false} />);
    await waitFor(() => expect(UrlSocket.urls).toHaveLength(1));
    fireEvent.click(screen.getByRole("radio", { name: "zsh" }));
    expect(window.localStorage.getItem("aurora.terminal.shell.agent-1")).toBe("zsh");
    expect(UrlSocket.urls).toHaveLength(1);
  });
});
