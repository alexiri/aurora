import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { type Handler, json, mockFetch } from "./support/api";
import { seedMoreControls } from "./support/render";

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
vi.mock("@/components/skills/skills-tab", () => ({ SkillsTab: () => <div data-testid="skills-tab" /> }));
vi.mock("@/components/agents/evals/evals-tab", () => ({ EvalsTab: () => <div data-testid="evals-tab" /> }));
vi.mock("@/components/peers/peers-tab", () => ({ PeersTab: () => <div data-testid="peers-tab" /> }));
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
  // The pages under test (Memory, Skills, Evals, Peers…) are Set up / Expert (D-406): the switch is on.
  seedMoreControls(qc);
  return render(
    <QueryClientProvider client={qc}>
      <AgentPage />
    </QueryClientProvider>,
  );
}
const sectionTabs = () => within(screen.getByRole("tablist", { name: "Agent sections" })).getAllByRole("tab").map((t) => t.textContent);
// (P5-REFINE-11, D-408) The labelled Settings button opens the Settings page directly (no menu).
const openSettings = async () => fireEvent.click(await screen.findByRole("button", { name: "Settings" }));
const settingsPages = () => within(screen.getByRole("tablist", { name: "Settings pages" })).getAllByRole("tab").map((t) => t.textContent);

describe("agent page frame (D-200)", () => {
  it("opens a hosted agent on Chat with three tabs in one header line and a labelled Settings button", async () => {
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
    // The old "…" menu is gone.
    expect(screen.queryByRole("button", { name: "Agent actions" })).toBeNull();
  });

  it("opens Settings on Instructions: no tab selected, a back link, the page name and its lead, the sub-nav; the URL keeps the page id", async () => {
    mockFetch(routes());
    renderPage();
    await screen.findByTestId("chat-tab");
    await openSettings();
    expect(await screen.findByTestId("instructions-tab")).toBeInTheDocument();
    expect(window.location.search).toBe("?tab=instructions");
    expect(within(screen.getByRole("tablist", { name: "Agent sections" })).queryByRole("tab", { selected: true })).toBeNull();
    expect(screen.getByRole("button", { name: "Settings" })).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("heading", { name: "Instructions", level: 2 })).toBeInTheDocument();
    expect(screen.getByText("Name, instructions, model")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Chat" })).toBeInTheDocument();
    // At Set up and Expert (the switch is on in this test) the sub-nav lists every page.
    expect(settingsPages()).toEqual(["Instructions", "Apps & access", "Memory", "Skills", "Tests", "Other agents"]);
    const pages = screen.getByRole("tablist", { name: "Settings pages" });
    fireEvent.click(within(pages).getByRole("tab", { name: "Memory" }));
    expect(await screen.findByTestId("memory-tab")).toBeInTheDocument();
    expect(window.location.search).toBe("?tab=memory");
    expect(screen.getByText("What it remembers")).toBeInTheDocument();
    fireEvent.click(within(pages).getByRole("tab", { name: "Apps & access" }));
    expect(await screen.findByTestId("access-tab")).toBeInTheDocument();
    expect(screen.getByText("Apps, accounts and the AI key it uses")).toBeInTheDocument();
    // Leaving and coming back with the Settings button returns to the page last open; the back link returns to Chat.
    fireEvent.click(screen.getByRole("button", { name: "Chat" }));
    expect(await screen.findByTestId("chat-tab")).toBeInTheDocument();
    await openSettings();
    expect(await screen.findByTestId("access-tab")).toBeInTheDocument();
  });

  it("opens old deep links on the Settings page they always opened: ?tab=memory, ?tab=access, ?tab=instructions, ?tab=skills, ?tab=evals, ?tab=peers", async () => {
    for (const [tab, testId, title] of [
      ["memory", "memory-tab", "Memory"],
      ["access", "access-tab", "Apps & access"],
      ["instructions", "instructions-tab", "Instructions"],
      ["skills", "skills-tab", "Skills"],
      ["evals", "evals-tab", "Tests"],
      ["peers", "peers-tab", "Other agents"],
    ] as const) {
      window.history.replaceState(null, "", `/?tab=${tab}`);
      mockFetch(routes());
      renderPage();
      expect(await screen.findByTestId(testId)).toBeInTheDocument();
      // A Settings page: no tab is selected, the page has its own title and the Settings button is highlighted.
      expect(screen.getByRole("heading", { name: title, level: 2 })).toBeInTheDocument();
      expect(within(screen.getByRole("tablist", { name: "Agent sections" })).queryByRole("tab", { selected: true })).toBeNull();
      expect(screen.getByRole("tab", { name: title, selected: true })).toBeInTheDocument();
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
    // Without a rail one chip stays in the header, in a person's words: never the raw kind, never "Default".
    expect(screen.getByText("Came with this workspace")).toBeInTheDocument();
    expect(screen.queryByText("byo")).toBeNull();
    expect(screen.queryByText("Default")).toBeNull();
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

describe("Settings danger zone (D-408)", () => {
  it("offers Pause Scout and Delete Scout… to owners at the bottom of every Settings page, never as a menu and not on Chat", async () => {
    mockFetch(routes());
    renderPage();
    await screen.findByTestId("chat-tab");
    expect(screen.queryByRole("button", { name: "Pause Scout" })).toBeNull();
    await openSettings();
    await screen.findByTestId("instructions-tab");
    expect(screen.getByRole("heading", { name: "Pause or delete" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Pause Scout" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Delete Scout…" })).toBeInTheDocument();
    expect(screen.getByText("Stops it until you resume it")).toBeInTheDocument();
    fireEvent.click(within(screen.getByRole("tablist", { name: "Settings pages" })).getByRole("tab", { name: "Memory" }));
    await screen.findByTestId("memory-tab");
    expect(screen.getByRole("button", { name: "Pause Scout" })).toBeInTheDocument();
    expect(screen.queryByRole("menuitem")).toBeNull();
  });

  it("keeps Pause behind its confirm dialog", async () => {
    const fn = mockFetch(routes({ "POST /v1/agents/a1/freeze": () => json(200, { frozen: null, effective_frozen: true }) }));
    renderPage();
    await openSettings();
    fireEvent.click(await screen.findByRole("button", { name: "Pause Scout" }));
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
    await openSettings();
    fireEvent.click(await screen.findByRole("button", { name: "Delete Scout…" }));
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

  it("a paused agent keeps its banner (with Resume) and Settings offers Resume Scout and Delete, not Pause", async () => {
    const frozen = { at: "2026-01-02T00:00:00Z", authority: "workspace_owner", by_label: "Ada", level: "agent", reason: "Runaway loop" };
    mockFetch(routes({}, { ...agent, frozen }));
    renderPage();
    expect(await screen.findByRole("button", { name: "Resume agent" })).toBeInTheDocument();
    await openSettings();
    expect(await screen.findByRole("button", { name: "Resume Scout" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Delete Scout…" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Pause Scout" })).toBeNull();
  });

  it("members without can_own see the Settings pages (read-only) and no Pause or Delete", async () => {
    mockFetch(
      routes({ "GET /v1/workspaces/w1": () => json(200, { id: "w1", org_id: "o1", my_role: "member", timezone: "Europe/Zurich" }) }),
    );
    renderPage();
    await screen.findByTestId("chat-tab");
    await openSettings();
    expect(await screen.findByTestId("instructions-tab")).toBeInTheDocument();
    expect(settingsPages()).toContain("Apps & access");
    expect(screen.queryByRole("button", { name: /^Pause/ })).toBeNull();
    expect(screen.queryByRole("button", { name: /^Delete/ })).toBeNull();
    expect(screen.queryByRole("heading", { name: "Pause or delete" })).toBeNull();
  });
});
