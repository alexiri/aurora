import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { AgentsView } from "@/components/agents/agents-view";
import { json, mockFetch } from "./support/api";
import { wrap } from "./support/render";

// (FIX-251, D-488(a)) The Agents page: the **Needs update** filter (`?needs_update=1`), the
// **Review updates** affordance, and the **Update available** mark on each agent behind.

const state = vi.hoisted(() => ({ search: "" }));

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
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
  state.search = "";
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
});
