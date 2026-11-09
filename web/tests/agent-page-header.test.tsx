import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "ops", handle: "scout" }),
  useRouter: () => ({ replace: vi.fn(), push: vi.fn() }),
  usePathname: () => "/o/acme/w/ops/agents/scout",
  useSearchParams: () => new URLSearchParams(),
}));
// The tabs have their own tests; here only the page header matters.
vi.mock("@/components/agents/chat-tab", () => ({ ChatTab: () => <div data-testid="chat-tab" /> }));
vi.mock("@/components/agents/instructions-tab", () => ({ InstructionsTab: () => <div data-testid="instructions-tab" /> }));
vi.mock("@/components/agents/delete-zone", () => ({ DeleteZone: () => null }));

import { AgentPage } from "@/components/agents/agent-page";

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
type Handler = (req: Request) => Response | Promise<Response>;
function mockFetch(routes: Record<string, Handler>) {
  const fn = vi.fn(async (input: Request | string | URL) => {
    const req = input as Request;
    const url = new URL(req.url, "http://localhost");
    const h = routes[`${req.method} ${url.pathname}`];
    return h ? h(req) : json(404, { error: { code: "not_found", message: "nf" } });
  });
  vi.stubGlobal("fetch", fn);
  return fn;
}
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const agent = {
  id: "a1", handle: "scout", name: "Scout", kind: "hosted", is_default: false, avatar_url: null, brief: null,
  frozen: null, updated_at: "2026-01-01T00:00:00Z",
};

function routes(extra: Record<string, Handler> = {}) {
  return {
    "GET /v1/resolve": () => json(200, { org: { id: "o1" }, workspace: { id: "w1" } }),
    "GET /v1/workspaces/w1/agents": () => json(200, { items: [agent], next_cursor: null }),
    "GET /v1/workspaces/w1": () => json(200, { id: "w1", org_id: "o1", my_role: "owner", timezone: "Europe/Zurich" }),
    "GET /v1/orgs/o1": () => json(200, { id: "o1", my_role: "owner", settings: {} }),
    ...extra,
  };
}

function renderPage() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <AgentPage />
    </QueryClientProvider>,
  );
}

describe("agent page header", () => {
  it("uses the compact header on the Chat tab only, with Freeze in the overflow menu", async () => {
    mockFetch(routes());
    renderPage();
    // Default tab is Instructions: the full header, with the Freeze button in the page.
    expect(await screen.findByTestId("instructions-tab")).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1 }).className).toContain("text-[30px]");
    expect(await screen.findByRole("button", { name: "Freeze agent" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Agent actions" })).toBeNull();

    fireEvent.click(screen.getByRole("tab", { name: "Chat" }));
    expect(await screen.findByTestId("chat-tab")).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1 }).className).not.toContain("text-[30px]");
    expect(screen.getByRole("heading", { level: 1 }).className).toContain("text-xl");
    expect(screen.getByRole("button", { name: "Agent actions" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Freeze agent" })).toBeNull();

    // Back to another tab: the header is the full one again.
    fireEvent.click(screen.getByRole("tab", { name: "Instructions" }));
    expect(screen.getByRole("heading", { level: 1 }).className).toContain("text-[30px]");
    expect(await screen.findByRole("button", { name: "Freeze agent" })).toBeInTheDocument();
  });

  it("keeps Freeze reachable from the overflow menu, behind its confirm dialog", async () => {
    const fn = mockFetch(
      routes({ "POST /v1/agents/a1/freeze": () => json(200, { frozen: null, effective_frozen: true }) }),
    );
    renderPage();
    fireEvent.click(await screen.findByRole("tab", { name: "Chat" }));
    const trigger = await screen.findByRole("button", { name: "Agent actions" });
    fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false });
    fireEvent.keyDown(trigger, { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: /Freeze agent/ }));

    // Nothing is sent until the dialog is confirmed with a reason.
    expect(await screen.findByRole("dialog", { name: /Freeze agent Scout\?/ })).toBeInTheDocument();
    expect(fn.mock.calls.some(([r]) => r instanceof Request && r.method === "POST")).toBe(false);
    const submit = screen.getByRole("button", { name: "Freeze agent" });
    expect(submit).toBeDisabled();
    fireEvent.change(screen.getByLabelText(/Reason/), { target: { value: "Runaway loop" } });
    fireEvent.click(submit);
    await waitFor(() =>
      expect(fn.mock.calls.some(([r]) => r instanceof Request && r.method === "POST" && r.url.endsWith("/v1/agents/a1/freeze"))).toBe(true),
    );
  });

  it("members without can_own get no Freeze entry in the Chat header", async () => {
    mockFetch(
      routes({ "GET /v1/workspaces/w1": () => json(200, { id: "w1", org_id: "o1", my_role: "member", timezone: "Europe/Zurich" }) }),
    );
    renderPage();
    fireEvent.click(await screen.findByRole("tab", { name: "Chat" }));
    await screen.findByTestId("chat-tab");
    expect(screen.queryByRole("button", { name: "Agent actions" })).toBeNull();
  });
});
