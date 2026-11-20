import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { type Handler, json, mockFetch } from "./support/api";

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

afterEach(() => {
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
  it("opens a hosted agent on Chat with three tabs in one header line; Instructions and Apps & access are in the menu", async () => {
    mockFetch(routes());
    renderPage();
    expect(await screen.findByTestId("chat-tab")).toBeInTheDocument();
    expect(sectionTabs()).toEqual(["Chat", "Automations", "Computer"]);
    expect(screen.getByRole("tab", { name: "Chat", selected: true })).toBeInTheDocument();
    const h1 = screen.getByRole("heading", { level: 1 });
    expect(h1.className).toContain("text-xl");
    expect(h1.textContent).toContain("Scout");
    expect(screen.getByText("@scout")).toBeInTheDocument();
    // Chat has a single page: no second row of tabs.
    expect(screen.queryByRole("tablist", { name: /pages/ })).toBeNull();
  });

  it("opens Instructions and Apps & access from the … menu: no tab selected, a back link, the page name and its sentence; the URL keeps the page id", async () => {
    mockFetch(routes());
    renderPage();
    await screen.findByTestId("chat-tab");
    await openMenu();
    fireEvent.click(await screen.findByRole("menuitem", { name: /Instructions/ }));
    expect(await screen.findByTestId("instructions-tab")).toBeInTheDocument();
    expect(window.location.search).toBe("?tab=instructions");
    expect(within(screen.getByRole("tablist", { name: "Agent sections" })).queryByRole("tab", { selected: true })).toBeNull();
    expect(screen.getByRole("heading", { name: "Instructions", level: 2 })).toBeInTheDocument();
    expect(screen.getByText("How Scout works: its name, instructions, model, memory and skills.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Chat" })).toBeInTheDocument();
    const pages = screen.getByRole("tablist", { name: "Instructions pages" });
    expect(within(pages).getAllByRole("tab").map((t) => t.textContent)).toEqual(["Instructions", "Memory", "Skills", "Tests"]);
    fireEvent.click(within(pages).getByRole("tab", { name: "Memory" }));
    expect(await screen.findByTestId("memory-tab")).toBeInTheDocument();
    expect(window.location.search).toBe("?tab=memory");
    // Apps & access from the same menu; leaving and coming back returns to the page last open in Instructions.
    await openMenu();
    fireEvent.click(await screen.findByRole("menuitem", { name: /Apps & access/ }));
    expect(await screen.findByTestId("access-tab")).toBeInTheDocument();
    expect(screen.getByText("What Scout can use: apps, accounts and its AI key.")).toBeInTheDocument();
    await openMenu();
    fireEvent.click(await screen.findByRole("menuitem", { name: /Instructions/ }));
    expect(await screen.findByTestId("memory-tab")).toBeInTheDocument();
    // The back link returns to Chat.
    fireEvent.click(screen.getByRole("button", { name: "Chat" }));
    expect(await screen.findByTestId("chat-tab")).toBeInTheDocument();
  });

  it("opens legacy deep links on the page they always opened: ?tab=memory, ?tab=access, ?tab=instructions", async () => {
    for (const [tab, testId, title] of [
      ["memory", "memory-tab", "Instructions"],
      ["access", "access-tab", "Apps & access"],
      ["instructions", "instructions-tab", "Instructions"],
    ] as const) {
      window.history.replaceState(null, "", `/?tab=${tab}`);
      mockFetch(routes());
      renderPage();
      expect(await screen.findByTestId(testId)).toBeInTheDocument();
      // The configuration page: no tab is selected and the page has its own title.
      expect(screen.getByRole("heading", { name: title, level: 2 })).toBeInTheDocument();
      expect(within(screen.getByRole("tablist", { name: "Agent sections" })).queryByRole("tab", { selected: true })).toBeNull();
      cleanup();
    }
  });

  it("opens Chat for a ?c= link, and a BYO agent on Instructions with Instructions · Computer · Apps & access", async () => {
    window.history.replaceState(null, "", "/?c=conv-1");
    mockFetch(routes());
    renderPage();
    expect(await screen.findByTestId("chat-tab")).toBeInTheDocument();
    cleanup();
    window.history.replaceState(null, "", "/");
    mockFetch(routes({}, { ...agent, kind: "byo", is_default: true }));
    renderPage();
    expect(await screen.findByTestId("instructions-tab")).toBeInTheDocument();
    expect(sectionTabs()).toEqual(["Computer"]);
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
  it("offers Pause and Delete agent… to owners, on every section, and no Delete zone under Instructions", async () => {
    mockFetch(routes());
    renderPage();
    await openMenu();
    expect(await screen.findByRole("menuitem", { name: /Instructions/ })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: /Apps & access/ })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: /Pause Scout/ })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: /Delete agent/ })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("menuitem", { name: /Instructions/ }));
    await screen.findByTestId("instructions-tab");
    expect(screen.queryByText("Delete this agent")).toBeNull();
    expect(screen.queryByRole("button", { name: "Delete agent" })).toBeNull();
    expect(screen.getByRole("button", { name: "Agent actions" })).toBeInTheDocument();
  });

  it("keeps Pause behind its confirm dialog", async () => {
    const fn = mockFetch(routes({ "POST /v1/agents/a1/freeze": () => json(200, { frozen: null, effective_frozen: true }) }));
    renderPage();
    await openMenu();
    fireEvent.click(await screen.findByRole("menuitem", { name: /Pause Scout/ }));
    expect(await screen.findByRole("dialog", { name: /Pause Scout\?/ })).toBeInTheDocument();
    expect(fn.mock.calls.some(([r]) => r instanceof Request && r.method === "POST")).toBe(false);
    const submit = screen.getByRole("button", { name: "Pause agent" });
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

  it("a paused agent keeps its banner (with Resume) and the menu offers Delete only", async () => {
    const frozen = { at: "2026-01-02T00:00:00Z", authority: "workspace_owner", by_label: "Ada", level: "agent", reason: "Runaway loop" };
    mockFetch(routes({}, { ...agent, frozen }));
    renderPage();
    expect(await screen.findByRole("button", { name: "Resume agent" })).toBeInTheDocument();
    await openMenu();
    expect(await screen.findByRole("menuitem", { name: /Delete agent/ })).toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: /Pause Scout/ })).toBeNull();
  });

  it("members without can_own see Instructions and Apps & access (read-only pages) and no Pause or Delete", async () => {
    mockFetch(
      routes({ "GET /v1/workspaces/w1": () => json(200, { id: "w1", org_id: "o1", my_role: "member", timezone: "Europe/Zurich" }) }),
    );
    renderPage();
    await screen.findByTestId("chat-tab");
    await openMenu();
    expect(await screen.findByRole("menuitem", { name: /Instructions/ })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: /Apps & access/ })).toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: /Pause/ })).toBeNull();
    expect(screen.queryByRole("menuitem", { name: /Delete/ })).toBeNull();
  });
});
