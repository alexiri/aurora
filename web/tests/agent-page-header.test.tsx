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
  // The pages under test (Evals, Peers…) are Expert (D-406; Memory and Skills are Use): the switch is on.
  seedMoreControls(qc);
  return render(
    <QueryClientProvider client={qc}>
      <AgentPage />
    </QueryClientProvider>,
  );
}
const sectionTabs = () => within(screen.getByRole("tablist", { name: "Agent sections" })).getAllByRole("tab").map((t) => t.textContent);
// (D-507) Settings is the fourth tab of the one header; there is no Settings button, back link or left sub-nav.
const agentTab = (name: string) => within(screen.getByRole("tablist", { name: "Agent sections" })).getByRole("tab", { name });
const openSettings = async () => fireEvent.click(await screen.findByRole("tab", { name: "Settings" }));
const settingsPages = () => within(screen.getByRole("tablist", { name: "Settings pages" })).getAllByRole("tab").map((t) => t.textContent);

describe("agent page frame (D-200, D-507)", () => {
  it("opens a hosted agent on Chat with one header and the tabs Chat · Automations · Computer · Settings", async () => {
    mockFetch(routes());
    renderPage();
    expect(await screen.findByTestId("chat-tab")).toBeInTheDocument();
    expect(sectionTabs()).toEqual(["Chat", "Automations", "Computer", "Settings"]);
    expect(screen.getByRole("tab", { name: "Chat", selected: true })).toBeInTheDocument();
    expect(agentTab("Chat")).toHaveAttribute("aria-current", "page");
    expect(agentTab("Settings")).not.toHaveAttribute("aria-current");
    expect(screen.queryByRole("button", { name: "Settings" })).toBeNull();
    // The tab row is part of the header, and the open section is its panel.
    expect(screen.getByRole("tablist", { name: "Agent sections" }).closest("header")).not.toBeNull();
    expect(screen.getByRole("tabpanel", { name: "Chat" })).toContainElement(screen.getByTestId("chat-tab"));
    const h1 = screen.getByRole("heading", { level: 1 });
    expect(h1.className).toContain("text-xl");
    expect(h1.textContent).toContain("Scout");
    expect(screen.getByText("@scout")).toBeInTheDocument();
    // Chat has a single page: no second row of tabs.
    expect(screen.queryByRole("tablist", { name: /pages/ })).toBeNull();
    // The old "…" menu is gone.
    expect(screen.queryByRole("button", { name: "Agent actions" })).toBeNull();
  });

  it("(access-3) the give-key action is its own full-width row at every width, and the name and the state line stay in the DOM", async () => {
    mockFetch(
      routes({
        "GET /v1/agents/a1/answer-readiness": () =>
          json(200, {
            can_answer: false,
            reason: "no_ai_key",
            usable_keys: 0,
            fix: { action: "give_key", key: { secret_id: "s1", name: "Company key", scope: "organization" } },
          }),
        "GET /v1/agents/a1/sandbox": () => json(200, { computer: { word: "on", ready: true, working: false, asleep: false } }),
      }),
    );
    renderPage();
    const give = await screen.findByRole("button", { name: "Give Scout the Company key" });
    // jsdom has no layout: the row's classes are the contract (basis-full + w-full below md too, never md-only), so it can never
    // join the name's flex line and squeeze the h1 to 0 px (Playwright's no-model spec measures the boxes at 390 and 1280).
    const row = give.closest("header > div")!;
    expect(row.className).toMatch(/(^|\s)basis-full(\s|$)/);
    expect(row.className).toMatch(/(^|\s)w-full(\s|$)/);
    expect(row.className).not.toContain("md:basis-full");
    const h1 = screen.getByRole("heading", { level: 1 });
    expect(h1.textContent).toBe("Scout");
    expect(await screen.findByTestId("agent-state")).toHaveTextContent("Can't answer: no AI key");
    expect(row.contains(h1)).toBe(false);
    expect(row.contains(screen.getByTestId("agent-state"))).toBe(false);
  });

  it("opens the Settings tab on Instructions: its second row, the page name and lead, no back link; the URL keeps the page id", async () => {
    mockFetch(routes());
    renderPage();
    await screen.findByTestId("chat-tab");
    await openSettings();
    expect(await screen.findByTestId("instructions-tab")).toBeInTheDocument();
    expect(window.location.search).toBe("?tab=instructions");
    expect(agentTab("Settings")).toHaveAttribute("aria-selected", "true");
    expect(agentTab("Settings")).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("tabpanel", { name: "Settings" })).toContainElement(screen.getByTestId("instructions-tab"));
    expect(screen.getByRole("heading", { name: "Instructions", level: 2 })).toBeInTheDocument();
    expect(screen.getByText("Name, instructions, model")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Chat" })).toBeNull();
    expect(within(screen.getByRole("tablist", { name: "Settings pages" })).getByRole("tab", { name: "Instructions" })).toHaveAttribute("aria-current", "page");
    // At Set up and Expert (the switch is on in this test) the second row lists every page.
    expect(settingsPages()).toEqual(["Instructions", "Apps & access", "Memory", "Skills", "Tests", "Other agents", "Manage"]);
    const pages = screen.getByRole("tablist", { name: "Settings pages" });
    fireEvent.click(within(pages).getByRole("tab", { name: "Memory" }));
    expect(await screen.findByTestId("memory-tab")).toBeInTheDocument();
    expect(window.location.search).toBe("?tab=memory");
    expect(screen.getByText("What it remembers")).toBeInTheDocument();
    fireEvent.click(within(pages).getByRole("tab", { name: "Apps & access" }));
    expect(await screen.findByTestId("access-tab")).toBeInTheDocument();
    expect(screen.getByText("Apps, accounts and the AI key it uses")).toBeInTheDocument();
    // Leaving by the Chat tab and coming back by the Settings tab returns to the page last open.
    fireEvent.click(agentTab("Chat"));
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
      // A Settings page: the Settings tab and the page's pill are selected, and the page has its own title.
      expect(screen.getByRole("heading", { name: title, level: 2 })).toBeInTheDocument();
      expect(agentTab("Settings")).toHaveAttribute("aria-selected", "true");
      expect(screen.getByRole("tab", { name: title, selected: true })).toBeInTheDocument();
      cleanup();
    }
  });

  it("(FIX-236) an emergency reader gets Chat · Automations · Settings with the pages they may read, and no write control in the frame", async () => {
    window.history.replaceState(null, "", "/?tab=memory");
    mockFetch(
      routes(
        { "GET /v1/workspaces/w1": () => json(200, { id: "w1", org_id: "o1", my_role: null, emergency_reader: true, timezone: "Europe/Zurich" }) },
        { ...agent, needs_update: true },
      ),
    );
    renderPage();
    expect(await screen.findByTestId("memory-tab")).toBeInTheDocument();
    expect(sectionTabs()).toEqual(["Chat", "Automations", "Settings"]);
    expect(settingsPages()).toEqual(["Instructions", "Memory", "Skills"]);
    expect(screen.queryByRole("button", { name: "Status" })).toBeNull();
    expect(screen.queryByRole("link", { name: /Update available/ })).toBeNull();
    expect(screen.queryByTestId("agent-state")).toBeNull();
    // The header holds its two links and the tabs, nothing that acts.
    expect(within(screen.getByRole("banner", { hidden: true })).queryByRole("button")).toBeNull();
  });

  it("the Computer tab has the same quiet second row, one line that never collapses into a More menu", async () => {
    window.history.replaceState(null, "", "/?tab=details");
    mockFetch(routes({ "GET /v1/instance": () => json(200, { features: { terminal: true, ssh: true } }) }));
    renderPage();
    const row = await screen.findByRole("tablist", { name: "Computer pages" });
    expect(agentTab("Computer")).toHaveAttribute("aria-current", "page");
    await waitFor(() => expect(within(row).getAllByRole("tab").map((t) => t.textContent)).toEqual(["Files", "Terminal", "Connect", "Details"]));
    expect(within(row).getByRole("tab", { name: "Details" })).toHaveAttribute("aria-current", "page");
    expect(screen.queryByRole("button", { name: /^More computer/ })).toBeNull();
    // Computer pages have no title row of their own: the state line comes straight under the row.
    expect(screen.queryByRole("heading", { level: 2, name: "Details" })).toBeNull();
  });

  it("opens Chat for a ?c= link, and a BYO agent on Instructions with the tabs Computer · Settings", async () => {
    window.history.replaceState(null, "", "/?c=conv-1");
    mockFetch(routes());
    renderPage();
    expect(await screen.findByTestId("chat-tab")).toBeInTheDocument();
    cleanup();
    window.history.replaceState(null, "", "/");
    mockFetch(routes({}, { ...agent, kind: "byo", is_default: true }));
    renderPage();
    expect(await screen.findByTestId("instructions-tab")).toBeInTheDocument();
    expect(sectionTabs()).toEqual(["Computer", "Settings"]);
    expect(agentTab("Settings")).toHaveAttribute("aria-selected", "true");
    expect(settingsPages()).toEqual(["Instructions", "Apps & access", "Manage"]);
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

/** Opens Settings, then the owner's last page, Manage (D-447). */
async function openManage() {
  await openSettings();
  await screen.findByTestId("instructions-tab");
  fireEvent.click(within(screen.getByRole("tablist", { name: "Settings pages" })).getByRole("tab", { name: "Manage" }));
  await screen.findByRole("heading", { name: "Delete", level: 2 });
}

describe("Settings Manage page (D-447)", () => {
  it("offers Pause Scout and Delete Scout… to owners on Manage only: no other Settings page, not Chat, never a menu", async () => {
    mockFetch(routes());
    renderPage();
    await screen.findByTestId("chat-tab");
    expect(screen.queryByRole("button", { name: "Pause Scout" })).toBeNull();
    await openSettings();
    await screen.findByTestId("instructions-tab");
    const pages = () => within(screen.getByRole("tablist", { name: "Settings pages" }));
    for (const [name, testId] of [["Instructions", "instructions-tab"], ["Memory", "memory-tab"], ["Skills", "skills-tab"], ["Tests", "evals-tab"], ["Other agents", "peers-tab"], ["Apps & access", "access-tab"]] as const) {
      fireEvent.click(pages().getByRole("tab", { name }));
      await screen.findByTestId(testId);
      expect(screen.queryByRole("button", { name: "Pause Scout" }), name).toBeNull();
      expect(screen.queryByRole("button", { name: "Delete Scout…" }), name).toBeNull();
      expect(screen.queryByRole("button", { name: /^Export Scout/ }), name).toBeNull();
      expect(screen.queryByRole("heading", { name: "Pause or delete" }), name).toBeNull();
    }
    fireEvent.click(pages().getByRole("tab", { name: "Manage" }));
    expect(await screen.findByRole("heading", { name: "Manage", level: 2 })).toBeInTheDocument();
    expect(screen.getByText("Pause, export, import or delete Scout")).toBeInTheDocument();
    expect(window.location.search).toBe("?tab=manage");
    expect(screen.getByRole("heading", { name: "Pause", level: 2 })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Pause Scout" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Export Scout…" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Import into Scout…" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Delete Scout…" })).toBeInTheDocument();
    expect(screen.getByText("Stops it until you resume it")).toBeInTheDocument();
    expect(screen.queryByRole("menuitem")).toBeNull();
  });

  it("opens ?tab=manage for an owner, and lands a member on the default page", async () => {
    window.history.replaceState(null, "", "/?tab=manage");
    mockFetch(routes());
    renderPage();
    expect(await screen.findByRole("button", { name: "Delete Scout…" })).toBeInTheDocument();
    cleanup();
    mockFetch(routes({ "GET /v1/workspaces/w1": () => json(200, { id: "w1", org_id: "o1", my_role: "member", timezone: "Europe/Zurich" }) }));
    renderPage();
    expect(await screen.findByTestId("chat-tab")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Delete Scout…" })).toBeNull();
  });

  it("keeps Pause behind its confirm dialog", async () => {
    const fn = mockFetch(routes({ "POST /v1/agents/a1/freeze": () => json(200, { frozen: null, effective_frozen: true }) }));
    renderPage();
    await openManage();
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
    await openManage();
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

  it("a paused agent keeps its banner (with Resume) and Manage offers Resume Scout and Delete, not Pause", async () => {
    const frozen = { at: "2026-01-02T00:00:00Z", authority: "workspace_owner", by_label: "Ada", level: "agent", reason: "Runaway loop" };
    mockFetch(routes({}, { ...agent, frozen }));
    renderPage();
    expect(await screen.findByRole("button", { name: "Resume agent" })).toBeInTheDocument();
    await openManage();
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
    expect(settingsPages()).not.toContain("Manage");
    expect(screen.queryByRole("button", { name: /^Pause/ })).toBeNull();
    expect(screen.queryByRole("button", { name: /^Delete/ })).toBeNull();
    expect(screen.queryByRole("heading", { name: "Pause or delete" })).toBeNull();
  });
});
