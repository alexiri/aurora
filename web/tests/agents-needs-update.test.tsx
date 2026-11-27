import { cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { AgentsView } from "@/components/agents/agents-view";
import { json, mockFetch } from "./support/api";
import { wrap } from "./support/render";

// (FIX-251, D-488(a)) The Agents page: the **Needs update** filter (`?needs_update=1`), the
// **Review updates** affordance, and the **Update available** mark on each agent behind.

const state = vi.hoisted(() => ({ search: "", pushed: [] as string[] }));

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: (href: string) => state.pushed.push(href), replace: vi.fn() }),
  useParams: () => ({ org: "acme", ws: "w1" }),
  useSearchParams: () => new URLSearchParams(state.search),
}));

const ORG = { id: "o1", slug: "acme", name: "Acme", my_role: "owner" };

const workspace = (over: Record<string, unknown> = {}) =>
  ({
    id: "w1", org_id: "o1", slug: "main", name: "Main", timezone: "UTC", memory_language: "english",
    byo_posture: null, my_role: "member", contacts: null,
    created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z", ...over,
  }) as never;

const agentRow = (over: Record<string, unknown> = {}) =>
  ({
    id: "a1", org_id: "o1", workspace_id: "w1", handle: "daneel", name: "Daneel", kind: "hosted",
    is_default: false, harness: null, model_provider: null, model_endpoint_id: null, model_id: null,
    effort: null, model_roles: null, subagent_model: "inherit", narration_cards: false,
    proactive_messaging: "ask", brief: "", avatar_url: null, byo: null, frozen: null,
    effective_frozen: false, template: null, needs_update: false,
    created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z", ...over,
  }) as never;

function mount(agents: unknown[]) {
  mockFetch({
    "GET /v1/resolve": () => json(200, { org: ORG, workspace: workspace() }),
    "GET /v1/workspaces/w1": () => json(200, workspace()),
    "GET /v1/workspaces/w1/agents": () => json(200, { items: agents, next_cursor: null }),
  });
  wrap(<AgentsView />, { moreControls: false });
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  state.search = "";
  state.pushed = [];
});

describe("the Agents page's Needs update filter (FIX-251)", () => {
  it("marks the agents that are behind, and offers the filter and Review updates", async () => {
    mount([
      agentRow({ needs_update: true }),
      agentRow({ id: "a2", handle: "kojo", name: "Kojo" }),
    ]);
    await screen.findByRole("heading", { name: "Agents" });
    expect(await screen.findByTestId("needs-update")).toHaveTextContent("1 agent needs an update.");
    // Both agents are listed; only the one behind is marked.
    expect(screen.getByTestId("agent-needs-update")).toBeInTheDocument();
    expect(screen.queryAllByTestId("agent-needs-update")).toHaveLength(1);
    expect(screen.getByRole("button", { name: "Review updates" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Needs update" })).toBeInTheDocument();
  });

  it("`?needs_update=1` lists only the agents that are behind, with the way back to all of them", async () => {
    state.search = "needs_update=1";
    mount([
      agentRow({ needs_update: true }),
      agentRow({ id: "a2", handle: "kojo", name: "Kojo" }),
    ]);
    await screen.findByRole("heading", { name: "Agents" });
    expect(await screen.findByTestId("needs-update")).toHaveTextContent("1 agent needs an update.");
    expect(screen.getByRole("link", { name: "Daneel" })).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Kojo" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Show all agents" })).toBeInTheDocument();
  });

  it("an empty filter says so, with the way back to all of them", async () => {
    state.search = "needs_update=1";
    mount([agentRow()]);
    await screen.findByRole("heading", { name: "Agents" });
    expect(await screen.findByTestId("needs-update")).toHaveTextContent("No agents need an update.");
    expect(screen.queryByRole("button", { name: "Review updates" })).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Daneel" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Show all agents" })).toBeInTheDocument();
  });

  // An apply refetches the agent list, which no longer marks the agent just updated: the
  // walkthrough still reviews every agent it started with, then ends on the unfiltered page.
  it("the walkthrough reviews every agent it started with while each apply shrinks the list of those behind", async () => {
    state.search = "needs_update=1";
    // jsdom has no `matchMedia`; the review dialog asks whether the viewport is wide.
    vi.stubGlobal("matchMedia", (query: string) => ({ matches: true, media: query, addEventListener: () => {}, removeEventListener: () => {} }));
    const applied = new Set<string>();
    const template = (version: string) => ({ template_id: "schedule-manager", name: "Schedule manager", version });
    const status = (id: string) => ({
      template: template(applied.has(id) ? "1.2.0" : "1.0.0"),
      available: true,
      notes: [],
      update: applied.has(id)
        ? null
        : { version: "1.2.0", content_sha256: "c".repeat(64), readme: "", changelog: null, inputs_needed: [], apps_added: [], parts: [] },
    });
    const apply = (id: string) => () => {
      applied.add(id);
      return json(200, status(id));
    };
    mockFetch({
      "GET /v1/resolve": () => json(200, { org: ORG, workspace: workspace() }),
      "GET /v1/workspaces/w1": () => json(200, workspace()),
      "GET /v1/workspaces/w1/agents": () =>
        json(200, {
          items: [
            agentRow({ needs_update: !applied.has("a1") }),
            agentRow({ id: "a2", handle: "kojo", name: "Kojo", needs_update: !applied.has("a2") }),
          ],
          next_cursor: null,
        }),
      "GET /v1/agents/a1/template": () => json(200, status("a1")),
      "GET /v1/agents/a2/template": () => json(200, status("a2")),
      "POST /v1/agents/a1/template/apply": apply("a1"),
      "POST /v1/agents/a2/template/apply": apply("a2"),
    });
    wrap(<AgentsView />, { moreControls: false });

    fireEvent.click(await screen.findByRole("button", { name: "Review updates" }));
    expect(await screen.findByRole("dialog", { name: "Review changes" })).toHaveTextContent("Daneel keeps working as it is");
    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    await waitFor(() => expect(screen.getByRole("dialog", { name: "Review changes" })).toHaveTextContent("Kojo keeps working as it is"));
    expect(state.pushed).toEqual([]);
    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    await waitFor(() => expect(state.pushed).toEqual(["/o/acme/w/w1/agents"]));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect([...applied]).toEqual(["a1", "a2"]);
  });
});
