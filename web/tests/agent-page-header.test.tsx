import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const router = vi.hoisted(() => ({ replace: vi.fn(), push: vi.fn() }));
vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "ops", handle: "scout" }),
  useRouter: () => router,
  usePathname: () => "/o/acme/w/ops/agents/scout",
  useSearchParams: () => new URLSearchParams(),
}));
// The pages have their own tests; here only the page frame matters.
vi.mock("@/components/agents/chat-tab", () => ({ ChatTab: () => <div data-testid="chat-tab" /> }));
vi.mock("@/components/agents/instructions-tab", () => ({ InstructionsTab: () => <div data-testid="instructions-tab" /> }));
vi.mock("@/components/memory/memory-tab", () => ({ MemoryTab: () => <div data-testid="memory-tab" /> }));
vi.mock("@/components/agents/access-tab", () => ({ AccessTab: () => <div data-testid="access-tab" /> }));
vi.mock("@/components/agents/connect-tab", () => ({ ConnectTab: () => <div data-testid="connect-tab" /> }));

import { AgentPage } from "@/components/agents/agent-page";

const json = (status: number, body: unknown) =>
  new Response(body === null ? null : JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
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
  window.history.replaceState(null, "", "/");
  router.replace.mockClear();
});

const agent = {
  id: "a1", handle: "scout", name: "Scout", kind: "hosted", is_default: false, avatar_url: null, brief: null,
  frozen: null, updated_at: "2026-01-01T00:00:00Z",
};

function routes(extra: Record<string, Handler> = {}, a: Record<string, unknown> = agent) {
  return {
    "GET /v1/resolve": () => json(200, { org: { id: "o1" }, workspace: { id: "w1" } }),
    "GET /v1/workspaces/w1/agents": () => json(200, { items: [a], next_cursor: null }),
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
const sectionTabs = () => within(screen.getByRole("tablist", { name: "Agent sections" })).getAllByRole("tab").map((t) => t.textContent);
const openMenu = async () => {
  const trigger = await screen.findByRole("button", { name: "Agent actions" });
  fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false });
  fireEvent.keyDown(trigger, { key: "Enter" });
};

describe("agent page frame (D-200)", () => {
  it("opens a hosted agent on Chat with the five sections in one header line", async () => {
    mockFetch(routes());
    renderPage();
    expect(await screen.findByTestId("chat-tab")).toBeInTheDocument();
    expect(sectionTabs()).toEqual(["Chat", "Behaviour", "Triggers", "Computer", "Access"]);
    expect(screen.getByRole("tab", { name: "Chat", selected: true })).toBeInTheDocument();
    const h1 = screen.getByRole("heading", { level: 1 });
    expect(h1.className).toContain("text-xl");
    expect(h1.textContent).toContain("Scout");
    expect(screen.getByText("@scout")).toBeInTheDocument();
    // Chat has a single page: no second row of tabs.
    expect(screen.queryByRole("tablist", { name: /pages/ })).toBeNull();
  });

  it("switches sections and shows a section's pages as a second row; the URL keeps the page id", async () => {
    mockFetch(routes());
    renderPage();
    await screen.findByTestId("chat-tab");
    fireEvent.click(screen.getByRole("tab", { name: "Behaviour" }));
    expect(await screen.findByTestId("instructions-tab")).toBeInTheDocument();
    expect(window.location.search).toBe("?tab=instructions");
    const pages = screen.getByRole("tablist", { name: "Behaviour pages" });
    expect(within(pages).getAllByRole("tab").map((t) => t.textContent)).toEqual(["Instructions", "Memory", "Skills"]);
    fireEvent.click(within(pages).getByRole("tab", { name: "Memory" }));
    expect(await screen.findByTestId("memory-tab")).toBeInTheDocument();
    expect(window.location.search).toBe("?tab=memory");
    // Leaving the section and coming back returns to the page last open in it.
    fireEvent.click(screen.getByRole("tab", { name: "Access" }));
    expect(await screen.findByTestId("access-tab")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("tab", { name: "Behaviour" }));
    expect(await screen.findByTestId("memory-tab")).toBeInTheDocument();
  });

  it("opens legacy deep links on the page they always opened: ?tab=memory, ?tab=access, ?tab=instructions", async () => {
    for (const [tab, testId, section] of [
      ["memory", "memory-tab", "Behaviour"],
      ["access", "access-tab", "Access"],
      ["instructions", "instructions-tab", "Behaviour"],
    ] as const) {
      window.history.replaceState(null, "", `/?tab=${tab}`);
      mockFetch(routes());
      renderPage();
      expect(await screen.findByTestId(testId)).toBeInTheDocument();
      expect(screen.getByRole("tab", { name: section, selected: true })).toBeInTheDocument();
      cleanup();
    }
  });

  it("opens Chat for a ?c= link, and a BYO agent on Instructions with Behaviour · Computer · Access", async () => {
    window.history.replaceState(null, "", "/?c=conv-1");
    mockFetch(routes());
    renderPage();
    expect(await screen.findByTestId("chat-tab")).toBeInTheDocument();
    cleanup();
    window.history.replaceState(null, "", "/");
    mockFetch(routes({}, { ...agent, kind: "byo", is_default: true }));
    renderPage();
    expect(await screen.findByTestId("instructions-tab")).toBeInTheDocument();
    expect(sectionTabs()).toEqual(["Behaviour", "Computer", "Access"]);
    // Without a rail the kind and Default badges stay in the header.
    expect(screen.getByText("byo")).toBeInTheDocument();
    expect(screen.getByText("Default")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("tab", { name: "Computer" }));
    expect(await screen.findByTestId("connect-tab")).toBeInTheDocument();
  });

  it("an unavailable ?tab= (Terminal off, Chat on a BYO agent) falls back to the default page", async () => {
    window.history.replaceState(null, "", "/?tab=terminal");
    mockFetch(routes());
    renderPage();
    expect(await screen.findByTestId("chat-tab")).toBeInTheDocument();
  });
});

describe("Agent actions menu", () => {
  it("offers Freeze and Delete agent… to owners, on every section, and no Delete zone under Instructions", async () => {
    mockFetch(routes());
    renderPage();
    await openMenu();
    expect(await screen.findByRole("menuitem", { name: /Freeze agent/ })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: /Delete agent/ })).toBeInTheDocument();
    fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });
    fireEvent.click(screen.getByRole("tab", { name: "Behaviour" }));
    await screen.findByTestId("instructions-tab");
    expect(screen.queryByText("Delete this agent")).toBeNull();
    expect(screen.queryByRole("button", { name: "Delete agent" })).toBeNull();
    expect(screen.getByRole("button", { name: "Agent actions" })).toBeInTheDocument();
  });

  it("keeps Freeze behind its confirm dialog", async () => {
    const fn = mockFetch(routes({ "POST /v1/agents/a1/freeze": () => json(200, { frozen: null, effective_frozen: true }) }));
    renderPage();
    await openMenu();
    fireEvent.click(await screen.findByRole("menuitem", { name: /Freeze agent/ }));
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

  it("deletes only after the handle is typed, then leaves for the agents list", async () => {
    const fn = mockFetch(routes({ "DELETE /v1/agents/a1": () => json(204, null) }));
    renderPage();
    await openMenu();
    fireEvent.click(await screen.findByRole("menuitem", { name: /Delete agent/ }));
    const dialog = await screen.findByRole("dialog", { name: "Delete this agent" });
    const submit = within(dialog).getByRole("button", { name: "Delete agent" });
    expect(submit).toBeDisabled();
    fireEvent.change(within(dialog).getByLabelText("Agent handle"), { target: { value: "scou" } });
    expect(submit).toBeDisabled();
    fireEvent.change(within(dialog).getByLabelText("Agent handle"), { target: { value: "scout" } });
    expect(fn.mock.calls.some(([r]) => r instanceof Request && r.method === "DELETE")).toBe(false);
    fireEvent.click(submit);
    await waitFor(() => expect(router.replace).toHaveBeenCalledWith("/o/acme/w/ops/agents"));
    expect(fn.mock.calls.some(([r]) => r instanceof Request && r.method === "DELETE" && r.url.endsWith("/v1/agents/a1"))).toBe(true);
  });

  it("a frozen agent keeps its banner (with Unfreeze) and the menu offers Delete only", async () => {
    const frozen = { at: "2026-01-02T00:00:00Z", authority: "workspace_owner", by_label: "Ada", level: "agent", reason: "Runaway loop" };
    mockFetch(routes({}, { ...agent, frozen }));
    renderPage();
    expect(await screen.findByRole("button", { name: "Unfreeze agent" })).toBeInTheDocument();
    await openMenu();
    expect(await screen.findByRole("menuitem", { name: /Delete agent/ })).toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: /Freeze agent/ })).toBeNull();
  });

  it("members without can_own get no Agent actions", async () => {
    mockFetch(
      routes({ "GET /v1/workspaces/w1": () => json(200, { id: "w1", org_id: "o1", my_role: "member", timezone: "Europe/Zurich" }) }),
    );
    renderPage();
    await screen.findByTestId("chat-tab");
    expect(screen.queryByRole("button", { name: "Agent actions" })).toBeNull();
  });
});
