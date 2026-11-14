import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { InstructionsTab } from "@/components/agents/instructions-tab";
import { TerminalTab } from "@/components/agents/terminal-tab";

// FIX-138 (D-246): the terminal's shell. The choice lives on the agent's Instructions page (owner 2026-11-14), remembered
// per browser and agent. bash is the server's default, so the default socket URL carries no query; zsh names itself
// with `?shell=zsh`; a change applies to the next session only (the open socket is not re-opened).

const { write } = vi.hoisted(() => ({ write: vi.fn() }));
vi.mock("next/navigation", () => ({ useParams: () => ({ org: "acme", ws: "main", handle: "daneel" }) }));
vi.mock("@wterm/ghostty", () => ({ GhosttyCore: { load: vi.fn(() => Promise.resolve({ dispose: vi.fn() })) } }));
vi.mock("@wterm/react", () => ({
  useTerminal: () => ({ ref: { current: null }, write }),
  Terminal: () => <div data-testid="wterm-pane" />,
}));
vi.mock("@/lib/api/conversations", () => ({
  useSandbox: () => ({ data: { state: "running" } }),
  useModels: () => ({ data: { items: [] }, isError: false }),
}));
vi.mock("@/components/agents/avatar-field", () => ({ AvatarField: () => null }));

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

const agent = (kind: "byo" | "hosted") => ({
  id: "agent-1", org_id: "o1", workspace_id: "w1", handle: "daneel", name: "Daneel", kind,
  is_default: false, harness: "claude-code", model_provider: null, model_endpoint_id: null, model_id: null,
  effort: null, proactive_messaging: "ask", brief: "", effective_frozen: false,
  created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z",
});

function wrap(ui: ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

beforeEach(() => {
  vi.stubGlobal("WebSocket", UrlSocket as unknown as typeof WebSocket);
  vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ items: [], default: "claude-code" }), { status: 200 })));
  UrlSocket.urls = [];
  window.localStorage.clear();
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("terminal pane reads the shell (FIX-138)", () => {
  it("connects without a query for bash and shows no shell picker", async () => {
    render(<TerminalTab agentId="agent-1" frozen={false} />);
    await waitFor(() => expect(UrlSocket.urls).toHaveLength(1));
    expect(UrlSocket.urls[0]).toBe("/v1/agents/agent-1/terminal");
    expect(screen.queryByRole("radiogroup", { name: "Shell" })).not.toBeInTheDocument();
  });

  it("connects with ?shell=zsh when zsh was chosen earlier in this browser", async () => {
    window.localStorage.setItem("aurora.terminal.shell.agent-1", "zsh");
    render(<TerminalTab agentId="agent-1" frozen={false} />);
    await waitFor(() => expect(UrlSocket.urls.length).toBeGreaterThan(0));
    expect(UrlSocket.urls.at(-1)).toBe("/v1/agents/agent-1/terminal?shell=zsh");
  });
});

describe("shell picker on the Instructions page (FIX-138, owner 2026-11-14)", () => {
  it("defaults to bash and remembers a new choice without Save", () => {
    wrap(<InstructionsTab agent={agent("hosted")} wsId="w1" terminal />);
    expect(screen.getByRole("radio", { name: "bash" })).toBeChecked();
    fireEvent.click(screen.getByRole("radio", { name: "zsh" }));
    expect(screen.getByRole("radio", { name: "zsh" })).toBeChecked();
    expect(window.localStorage.getItem("aurora.terminal.shell.agent-1")).toBe("zsh");
  });

  it("shows the remembered choice", () => {
    window.localStorage.setItem("aurora.terminal.shell.agent-1", "zsh");
    wrap(<InstructionsTab agent={agent("hosted")} wsId="w1" terminal />);
    expect(screen.getByRole("radio", { name: "zsh" })).toBeChecked();
  });

  it("is absent without the terminal, and for BYO agents", () => {
    wrap(<InstructionsTab agent={agent("hosted")} wsId="w1" />);
    expect(screen.queryByRole("radiogroup", { name: "Shell" })).not.toBeInTheDocument();
    cleanup();
    wrap(<InstructionsTab agent={agent("byo")} wsId="w1" terminal />);
    expect(screen.queryByRole("radiogroup", { name: "Shell" })).not.toBeInTheDocument();
  });
});
