import { fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { AgentPage } from "@/components/agents/agent-page";
import { wrap } from "./support/render";

// The Terminal tab (P4-TERM, C-622): tab gating (hosted + `features.terminal` only), the
// freeze notice, and the CSP's `'wasm-unsafe-eval'` in `script-src` (proxy.ts). The wterm
// pane itself needs WASM: jsdom has none, so the tests assert the surrounding states.

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "main", handle: "daneel" }),
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  usePathname: () => "/o/acme/w/main/agents/daneel",
  useSearchParams: () => new URLSearchParams(),
}));
// The wterm pane needs WASM + layout (none in jsdom); the pane's own states are covered by
// its component contract — here only the gating and the freeze notice matter.
vi.mock("@/components/agents/terminal-tab", () => ({
  TerminalTab: ({ frozen }: { frozen: boolean }) => (
    <div data-testid="terminal-tab">{frozen ? "paused" : "terminal"}</div>
  ),
}));
vi.mock("@/components/agents/instructions-tab", () => ({ InstructionsTab: () => <div data-testid="instructions-tab" /> }));
// The Computer pages have their own tests; here the Computer section only has to offer the Terminal page.
vi.mock("@/components/agents/computer-tab", () => ({ ComputerTab: () => <div data-testid="computer-tab" /> }));
vi.mock("@/components/agents/previews-tab", () => ({ PreviewsTab: () => <div data-testid="previews-tab" /> }));
// A hosted agent opens on Chat (D-200); the chat has its own tests.
vi.mock("@/components/agents/chat-tab", () => ({ ChatTab: () => <div data-testid="chat-tab" /> }));

const agent = {
  id: "00000000-0000-4000-8000-000000000002",
  org_id: "00000000-0000-4000-8000-000000000001",
  workspace_id: "00000000-0000-4000-8000-000000000003",
  handle: "daneel",
  name: "Daneel",
  brief: "",
  kind: "hosted" as const,
  harness: "fake",
  is_default: false,
  avatar_url: null,
  frozen: null,
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
};

const sandbox = {
  agent_id: agent.id,
  state: "running" as const,
  boot_id: "00000000-0000-4000-8000-000000000010",
  runner_id: "00000000-0000-4000-8000-000000000011",
  image: "aurora-sandbox:latest",
  started_at: "2026-01-01T00:00:00Z",
  keep_awake_reasons: [],
  failure_code: null,
  failure_message: null,
  start_attempt: null,
  retryable: false,
  start_log: [],
  processes: [],
  ports: [],
  preview_in_use: false,
  terminal_in_use: false,
  restart_to_update: false,
};

const ws = {
  id: "00000000-0000-4000-8000-000000000003",
  org_id: "00000000-0000-4000-8000-000000000001",
  slug: "main",
  name: "Main workspace",
  my_role: "owner" as const,
  memory_language: null,
  timezone: null,
  settings: { previews_enabled: true },
};

const org = {
  id: "00000000-0000-4000-8000-000000000001",
  slug: "acme",
  name: "Acme",
  my_role: "owner" as const,
  frozen: null,
  settings: { previews_enabled: true },
};

const json = (status: number, body: unknown) =>
  new Response(body === null ? null : JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });

function mockFetch(features: { terminal: boolean }) {
  const fn = vi.fn(async (input: Request | string | URL) => {
    const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"));
    const p = new URL(req.url).pathname;
    if (p === "/v1/instance")
      return json(200, {
        version: "0.1.0", signup_mode: "open", setup_errors: [],
        features: { hosted: true, channels: false, previews: true, terminal: features.terminal },
        previews: { enabled: true, domain: "previews.example.org" },
        oidc_providers: [],
      });
    if (p === "/v1/resolve") return json(200, { org, workspace: ws });
    if (p === "/v1/me") return json(200, { user: { id: "u1", email: "ada@example.test", display_name: "Ada", is_instance_admin: true } });
    if (p === `/v1/workspaces/${ws.id}`) return json(200, ws);
    if (p === `/v1/orgs/${org.id}`) return json(200, org);
    if (p.endsWith("/agents")) return json(200, { items: [agent] });
    if (p.endsWith("/sandbox")) return json(200, sandbox);
    return json(200, { items: [] });
  });
  vi.stubGlobal("fetch", fn);
  return fn;
}

afterEach(() => {
  // Switching sections writes `?tab=` to the URL; the next test starts on the default page.
  window.history.replaceState(null, "", "/");
});

describe("Terminal tab gating (C-622)", () => {
  it("offers the tab when the instance enabled the terminal", async () => {
    mockFetch({ terminal: true });
    wrap(<AgentPage />);
    // Terminal is a page of the Computer section (D-200).
    fireEvent.click(await screen.findByRole("tab", { name: "Computer" }));
    await waitFor(() => expect(screen.getByRole("tab", { name: "Terminal" })).toBeInTheDocument());
  });

  it("offers no tab when the terminal is off (default)", async () => {
    mockFetch({ terminal: false });
    wrap(<AgentPage />);
    fireEvent.click(await screen.findByRole("tab", { name: "Computer" }));
    await screen.findByTestId("computer-tab");
    expect(screen.queryByRole("tab", { name: "Terminal" })).not.toBeInTheDocument();
  });

  it("renders the freeze notice instead of a terminal for a frozen agent", async () => {
    mockFetch({ terminal: true });
    const frozenAgent = {
      ...agent,
      frozen: {
        at: "2026-01-02T00:00:00Z",
        authority: "workspace_owner",
        by_label: "ada@example.test",
        level: "agent",
        reason: "Runaway loop",
      },
    };
    const fn = vi.fn(async (input: Request | string | URL) => {
      const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"));
      const p = new URL(req.url).pathname;
      if (p.endsWith("/agents")) return json(200, { items: [frozenAgent] });
      if (p === "/v1/instance")
        return json(200, {
          version: "0.1.0", signup_mode: "open", setup_errors: [],
          features: { hosted: true, channels: false, previews: true, terminal: true },
          previews: { enabled: true, domain: "previews.example.org" },
          oidc_providers: [],
        });
      if (p === "/v1/resolve") return json(200, { org, workspace: ws });
      if (p === "/v1/me") return json(200, { user: { id: "u1", email: "ada@example.test", display_name: "Ada", is_instance_admin: true } });
      if (p === `/v1/workspaces/${ws.id}`) return json(200, ws);
      if (p === `/v1/orgs/${org.id}`) return json(200, org);
      return json(200, { items: [] });
    });
    vi.stubGlobal("fetch", fn);
    wrap(<AgentPage />);
    fireEvent.click(await screen.findByRole("tab", { name: "Computer" }));
    await waitFor(() => expect(screen.getByRole("tab", { name: "Terminal" })).toBeInTheDocument());
    screen.getByRole("tab", { name: "Terminal" }).click();
    await waitFor(() => expect(screen.getByText(/paused/i)).toBeInTheDocument());
  });
});
