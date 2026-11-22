import { fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ComputerTab } from "@/components/agents/computer-tab";
import { TerminalTab } from "@/components/agents/terminal-tab";
import { err, json, mockFetch } from "./support/api";
import { wrap } from "./support/render";

// FIX-138 (D-246), FIX-218 (D-459, C-1145): the terminal's shell. The choice lives on the agent's Computer page (owner
// 2026-11-14) and is stored on the server per person and agent, so the pane's socket URL never carries a query (the server
// applies the stored value) and SSH uses it too; the old per-browser `localStorage` value moves once.

const { write } = vi.hoisted(() => ({ write: vi.fn() }));
vi.mock("next/navigation", () => ({ useParams: () => ({ org: "acme", ws: "main", handle: "daneel" }) }));
vi.mock("@wterm/ghostty", () => ({ GhosttyCore: { load: vi.fn(() => Promise.resolve({ dispose: vi.fn() })) } }));
vi.mock("@wterm/react", () => ({
  useTerminal: () => ({ ref: { current: null }, write }),
  Terminal: () => <div data-testid="wterm-pane" />,
}));
const sandbox = {
  agent_id: "agent-1", state: "running", image: null, keep_awake_reasons: [], failure_code: null, failure_message: null,
  start_attempt: null, retryable: false, start_log: [], processes: [], ports: [], restart_to_update: false,
};
vi.mock("@/lib/api/conversations", () => ({
  useSandbox: () => ({ data: sandbox, isPending: false, isError: false }),
  useRestartSandbox: () => ({ isPending: false, error: null, mutate: vi.fn() }),
  useRetrySandbox: () => ({ isPending: false, error: null, mutate: vi.fn() }),
  // (P5-APIGAP-1) The Computer page's Wake button.
  useWakeSandbox: () => ({ isPending: false, error: null, mutate: vi.fn() }),
  useSleepSandbox: () => ({ isPending: false, error: null, mutate: vi.fn() }),
  useStartFailures: () => ({ data: { items: [] } }),
}));
vi.mock("@/lib/api/queries", () => ({
  useInstance: () => ({ data: undefined }),
  useResolve: () => ({ data: undefined }),
  useOrg: () => ({ data: undefined }),
  useMe: () => ({ data: undefined }),
  useWorkspace: () => ({ data: undefined }),
  useWorkspaces: () => ({ data: undefined, isPending: false }),
}));
vi.mock("@/components/agents/previews-tab", () => ({ ShareDialog: () => null }));

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

describe("terminal pane leaves the shell to the server (FIX-138, FIX-218)", () => {
  it("connects without a query and shows no shell picker", async () => {
    wrap(<TerminalTab agentId="agent-1" frozen={false} />);
    await waitFor(() => expect(UrlSocket.urls).toHaveLength(1));
    expect(UrlSocket.urls[0]).toBe("/v1/agents/agent-1/terminal");
    expect(screen.queryByRole("radiogroup", { name: "Shell" })).not.toBeInTheDocument();
  });

  it("never sends ?shell=, whatever this browser remembered (the server applies the stored choice)", async () => {
    window.localStorage.setItem("aurora.terminal.shell.agent-1", "zsh");
    wrap(<TerminalTab agentId="agent-1" frozen={false} />);
    await waitFor(() => expect(UrlSocket.urls.length).toBeGreaterThan(0));
    expect(UrlSocket.urls.at(-1)).toBe("/v1/agents/agent-1/terminal");
  });
});

const SHELL_PATH = "/v1/agents/agent-1/terminal/shell";
type Pref = { shell: "bash" | "zsh"; stored: boolean };

/** The preference routes: `GET` answers `pref`; `PUT` records its body and answers `put` (default: the saved value). */
function mockPref(pref: Pref, put?: () => Response) {
  const puts: unknown[] = [];
  mockFetch({
    [`GET ${SHELL_PATH}`]: () => json(200, pref),
    [`PUT ${SHELL_PATH}`]: async (req) => {
      const body = (await req.json()) as { shell: "bash" | "zsh" };
      puts.push(body);
      return put ? put() : json(200, { shell: body.shell, stored: true });
    },
  });
  return puts;
}

describe("shell picker on the Computer page (FIX-138, owner 2026-11-14; stored on the server, FIX-218)", () => {
  it("shows bash while nothing is stored, with the SSH line", async () => {
    mockPref({ shell: "bash", stored: false });
    wrap(<ComputerTab agentId="agent-1" terminal />);
    await waitFor(() => expect(screen.getByRole("radio", { name: "bash" })).toBeEnabled());
    expect(screen.getByRole("radio", { name: "bash" })).toBeChecked();
    expect(screen.getByText("Also used when you connect with SSH.")).toBeInTheDocument();
  });

  it("shows the stored choice", async () => {
    mockPref({ shell: "zsh", stored: true });
    wrap(<ComputerTab agentId="agent-1" terminal />);
    await waitFor(() => expect(screen.getByRole("radio", { name: "zsh" })).toBeChecked());
  });

  it("saves a change through the API at once and shows it", async () => {
    const puts = mockPref({ shell: "bash", stored: false });
    wrap(<ComputerTab agentId="agent-1" terminal />);
    await waitFor(() => expect(screen.getByRole("radio", { name: "zsh" })).toBeEnabled());
    fireEvent.click(screen.getByRole("radio", { name: "zsh" }));
    await waitFor(() => expect(puts).toEqual([{ shell: "zsh" }]));
    await waitFor(() => expect(screen.getByRole("radio", { name: "zsh" })).toBeChecked());
    expect(window.localStorage.getItem("aurora.terminal.shell.agent-1")).toBeNull();
  });

  it("shows a failed save inline and falls back to the stored value", async () => {
    mockPref({ shell: "bash", stored: false }, () => err(500, "internal", "Could not save."));
    wrap(<ComputerTab agentId="agent-1" terminal />);
    await waitFor(() => expect(screen.getByRole("radio", { name: "zsh" })).toBeEnabled());
    fireEvent.click(screen.getByRole("radio", { name: "zsh" }));
    expect(await screen.findByRole("alert")).toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole("radio", { name: "bash" })).toBeChecked());
  });

  it("moves a remembered zsh to the server once when nothing is stored, and removes the old key", async () => {
    window.localStorage.setItem("aurora.terminal.shell.agent-1", "zsh");
    const puts = mockPref({ shell: "bash", stored: false });
    wrap(<ComputerTab agentId="agent-1" terminal />);
    await waitFor(() => expect(puts).toEqual([{ shell: "zsh" }]));
    expect(window.localStorage.getItem("aurora.terminal.shell.agent-1")).toBeNull();
    await waitFor(() => expect(screen.getByRole("radio", { name: "zsh" })).toBeChecked());
    expect(puts).toHaveLength(1);
  });

  it("removes the old key without saving when the server already has a choice, or the old value was bash", async () => {
    for (const [legacy, pref] of [
      ["zsh", { shell: "bash", stored: true }],
      ["bash", { shell: "bash", stored: false }],
    ] as const) {
      window.localStorage.setItem("aurora.terminal.shell.agent-1", legacy);
      const puts = mockPref(pref);
      const view = wrap(<ComputerTab agentId="agent-1" terminal />);
      await waitFor(() => expect(window.localStorage.getItem("aurora.terminal.shell.agent-1")).toBeNull());
      await waitFor(() => expect(screen.getByRole("radio", { name: "bash" })).toBeEnabled());
      expect(puts).toEqual([]);
      view.unmount();
    }
  });

  it("is absent when the instance has no terminal", () => {
    wrap(<ComputerTab agentId="agent-1" />);
    // Details (D-412): the facts a member reads; Processes and the rest are the admins' fold.
    expect(screen.getByTestId("computer-facts")).toBeInTheDocument();
    expect(screen.queryByRole("radiogroup", { name: "Shell" })).not.toBeInTheDocument();
  });
});
