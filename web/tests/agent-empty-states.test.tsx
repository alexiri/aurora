import { screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { EvalsTab } from "@/components/agents/evals/evals-tab";
import { AgentChannelsTab } from "@/components/channels/agent-channels-tab";
import { MemoryTab } from "@/components/memory/memory-tab";
import { SkillsTab } from "@/components/skills/skills-tab";
import { WebhooksTab } from "@/components/webhooks/webhooks-tab";
import { json } from "./support/api";
import { wrap } from "./support/render";

// (P5-POLISH-6, C6) The empty states of the agent's pages: one title, one sentence, one action (or who can, with the
// pane for "Select a … to edit it" hidden while the list is empty).

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "main" }),
  usePathname: () => "/o/acme/w/main/agents/daneel",
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useSearchParams: () => new URLSearchParams(),
}));

type Handler = (req: Request) => Response | Promise<Response>;
function mockFetch(routes: Record<string, Handler>) {
  const fn = vi.fn(async (input: Request | string | URL) => {
    const req = input as Request;
    const url = new URL(req.url, "http://localhost");
    const h = routes[`${req.method} ${decodeURIComponent(url.pathname)}`];
    return h ? h(req) : json(404, { error: { code: "not_found", message: "nf" } });
  });
  vi.stubGlobal("fetch", fn);
  return fn;
}
const page = (items: unknown[] = []) => () => json(200, { items, next_cursor: null });

describe("empty states on the agent's pages", () => {
  it("Memory: 'Daneel doesn't remember anything yet', New memory, and no Select-a-memory pane", async () => {
    mockFetch({ "GET /v1/agents/a1/memories": page() });
    wrap(<MemoryTab agentId="a1" agentName="Daneel" />);
    expect(await screen.findByRole("heading", { name: "Daneel doesn't remember anything yet" })).toBeInTheDocument();
    expect(screen.getByText("It saves notes here as you work together. You can also write one yourself.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "New memory" })).toBeInTheDocument();
    expect(screen.queryByText(/Select a memory/)).toBeNull();
  });

  it("Skills: the same pattern, no Select-a-skill pane, and 'Skills this agent receives' only when there are some", async () => {
    mockFetch({
      "GET /v1/agents/a1/skills": page(),
      "GET /v1/agents/a1/skills/effective": () => json(200, { items: [], shadowed: [] }),
    });
    wrap(<SkillsTab agentId="a1" />);
    expect(await screen.findByRole("heading", { name: "No skills yet" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "New skill" })).toBeInTheDocument();
    expect(screen.queryByText(/Select a skill/)).toBeNull();
    expect(screen.queryByText("Skills this agent receives")).toBeNull();
  });

  it("Webhooks: the empty state has New webhook", async () => {
    mockFetch({ "GET /v1/agents/a1/webhooks": page() });
    wrap(<WebhooksTab agentId="a1" canOwn />);
    expect(await screen.findByRole("heading", { name: "No webhooks" })).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "New webhook" }).length).toBeGreaterThan(0);
  });

  it("Channels: with no chat platform the whole tab is one empty state and none of the sections show", async () => {
    mockFetch({
      "GET /v1/connectors": () => json(200, { items: [] }),
      "GET /v1/agents/a1/presences": page(),
    });
    wrap(<AgentChannelsTab agentId="a1" agentName="Daneel" orgId="o1" canOwn wsId="w1" />);
    expect(await screen.findByRole("heading", { name: "Daneel isn't in any chat yet" })).toBeInTheDocument();
    expect(screen.getByText("Your organization hasn't connected a chat platform. Once it has, you can add Daneel to channels here.")).toBeInTheDocument();
    expect(screen.queryByText("Presences")).toBeNull();
    expect(screen.queryByText("Space bindings")).toBeNull();
    expect(screen.queryByText("Reach requests")).toBeNull();
    expect(screen.queryByText(/Show progress in chat/)).toBeNull();
  });

  it("Evals: a fresh agent shows one empty state; Judge, Runs and Schedules wait for the first question", async () => {
    mockFetch({ "GET /v1/agents/a1/eval-questions": () => json(200, { questions: [] }) });
    wrap(<EvalsTab agentId="a1" frozen={false} canOwn />);
    expect(await screen.findByRole("heading", { name: "No questions yet" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Add a question" })).toBeInTheDocument();
    expect(screen.queryByText("Judge")).toBeNull();
    expect(screen.queryByText("Runs")).toBeNull();
  });
});
