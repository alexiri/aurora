import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { json } from "./support/api";

vi.mock("next/navigation", () => ({
  usePathname: () => "/x",
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useSearchParams: () => new URLSearchParams(window.location.search),
  useParams: () => ({ org: "acme", ws: "main", handle: "scout" }),
}));

import { UsageCard } from "@/components/usage/usage-card";
import { UsageView, type UsageLevel } from "@/components/usage/usage-view";
import { presetRange } from "@/lib/usage-format";

// P5-USAGE-2 (C-1000, C-1003, C-1004, C-1006; 14 §Usage): the one view in its three levels.

const z = { turns: 0, turn_seconds: 0, awake_seconds: 0, input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0 };
const c = (o: Partial<typeof z>) => ({ ...z, ...o });

function days(from: string, to: string, at: Record<string, Partial<typeof z>> = {}) {
  const out = [];
  for (let d = new Date(`${from}T00:00:00Z`); d <= new Date(`${to}T00:00:00Z`); d = new Date(d.getTime() + 86_400_000)) {
    const day = d.toISOString().slice(0, 10);
    out.push({ day, ...c(at[day] ?? {}) });
  }
  return out;
}

const TOTALS = c({ turns: 5, turn_seconds: 1500, awake_seconds: 12_000, input_tokens: 1_000_000, output_tokens: 234_567, cache_read_tokens: 10, cache_write_tokens: 20 });
const MODELS = [
  { provider: "litellm", endpoint_id: "ep", model: "glm-4.6", turns: 5, turn_seconds: 1500, input_tokens: 900_000, output_tokens: 200_000, cache_read_tokens: 10, cache_write_tokens: 20 },
  { provider: null, endpoint_id: null, model: null, turns: 0, turn_seconds: 0, input_tokens: 100_000, output_tokens: 34_567, cache_read_tokens: 0, cache_write_tokens: 0 },
];

interface Calls {
  paths: string[];
}
function mock(handler: (url: URL) => Response | null) {
  const calls: Calls = { paths: [] };
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const url = new URL((input as Request).url, "http://localhost");
      calls.paths.push(url.pathname + url.search);
      const r = handler(url);
      if (r) return r;
      if (url.pathname === "/v1/workspaces/w1/agents") return json(200, { items: [{ id: "a1", name: "Scout", handle: "scout" }, { id: "a2", name: "Ada", handle: "ada" }], next_cursor: null });
      if (url.pathname === "/v1/orgs/o1/workspaces") return json(200, { items: [{ id: "w1", name: "Main", slug: "main" }], next_cursor: null });
      if (url.pathname === "/v1/admin/orgs") return json(200, { items: [{ id: "o1", name: "Acme", slug: "acme" }], next_cursor: null });
      return json(200, { items: [] });
    }),
  );
  return calls;
}

afterEach(() => {
  window.history.replaceState(null, "", "/");
});

function renderLevel(level: UsageLevel) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <UsageView level={level} />
    </QueryClientProvider>,
  );
}
const WS: UsageLevel = { kind: "workspace", wsId: "w1", base: "/o/acme/w/main" };

function workspaceBody(from: string, to: string, extra: Record<string, unknown> = {}) {
  return {
    from,
    to,
    totals: TOTALS,
    by_day: days(from, to, { [to]: { input_tokens: 1000, output_tokens: 500, turns: 2 } }),
    by_agent: [
      { agent_id: "a1", agent_name: "Scout", ...c({ input_tokens: 900_000, turns: 3 }) },
      { agent_id: "gone", agent_name: null, ...c({ input_tokens: 100_000, turns: 2 }) },
    ],
    by_model: MODELS,
    runtime_as_of: null,
    ...extra,
  };
}

describe("UsageView in a workspace", () => {
  it("opens on 30 days with every day present, the four tiles, the opening sentence and the tables", async () => {
    const calls = mock((u) => {
      if (u.pathname !== "/v1/workspaces/w1/usage") return null;
      return json(200, workspaceBody(u.searchParams.get("from")!, u.searchParams.get("to")!));
    });
    renderLevel(WS);
    const r = presetRange(30);
    await screen.findByText("Tokens per day");
    expect(calls.paths.some((p) => p.includes(`from=${r.from}`) && p.includes(`to=${r.to}`))).toBe(true);
    expect(screen.getByText(/How much your agents used: AI tokens, the turns they answered, and how long their computers were awake\. Days are in UTC\./)).toBeInTheDocument();
    // four tiles: tokens (input + output) compact with the exact value in the title
    expect(screen.getByText("1.2 M")).toHaveAttribute("title", "1,234,567");
    expect(screen.getByText(/Input 1\.0? ?M|Input 1 M/)).toBeInTheDocument();
    expect(screen.getAllByText("Turns").length).toBeGreaterThan(1);
    expect(screen.getAllByText("25 min").length).toBeGreaterThan(0); // turn time (tile and tables)
    expect(screen.getByText("3 h 20 min")).toBeInTheDocument(); // awake time
    // zero days are present: 30 bars named in the chart
    const chart = screen.getByTestId("bar-chart");
    expect(within(chart).getAllByRole("listitem")).toHaveLength(30);
    // a null runtime_as_of
    expect(screen.getByText("Not counted yet")).toBeInTheDocument();
    // tables: a deleted agent and an unknown model
    expect(screen.getByRole("link", { name: "Scout" })).toHaveAttribute("href", "/o/acme/w/main/agents/a1");
    expect(screen.getByText("Deleted agent")).toBeInTheDocument();
    expect(screen.getByText("Unknown model")).toBeInTheDocument();
    expect(screen.getByText(/Turns count under the agent's main model; a model used only by helpers shows tokens and no turns\./)).toBeInTheDocument();
  });

  it("shows when the runtime was last counted", async () => {
    mock((u) =>
      u.pathname === "/v1/workspaces/w1/usage" ? json(200, workspaceBody(u.searchParams.get("from")!, u.searchParams.get("to")!, { runtime_as_of: "2026-11-17T10:00:00Z" })) : null,
    );
    renderLevel(WS);
    await screen.findByText(/^As of /);
    expect(screen.queryByText("Not counted yet")).toBeNull();
  });

  it("switching the range updates the URL and the query", async () => {
    const calls = mock((u) => (u.pathname === "/v1/workspaces/w1/usage" ? json(200, workspaceBody(u.searchParams.get("from")!, u.searchParams.get("to")!)) : null));
    renderLevel(WS);
    await screen.findByText("Tokens per day");
    fireEvent.click(screen.getByLabelText("7 days"));
    expect(window.location.search).toContain("range=7d");
    const r = presetRange(7);
    await waitFor(() => expect(calls.paths.some((p) => p.includes(`from=${r.from}`))).toBe(true));
    await waitFor(() => expect(within(screen.getByTestId("bar-chart")).getAllByRole("listitem")).toHaveLength(7));
    fireEvent.click(screen.getByLabelText("30 days"));
    expect(window.location.search).toContain("range=30d");
  });

  it("a custom range over 366 days shows the API's message on the field", async () => {
    mock((u) =>
      u.pathname === "/v1/workspaces/w1/usage"
        ? json(422, { error: { code: "usage_range_invalid", message: "The range is longer than 366 days.", details: { reason: "too_long" } } })
        : null,
    );
    window.history.replaceState(null, "", "/x?from=2024-01-01&to=2026-01-01");
    renderLevel(WS);
    expect(await screen.findByText("The range is longer than 366 days.")).toBeInTheDocument();
    expect(screen.getByLabelText("From (UTC)")).toHaveAttribute("aria-invalid", "true");
    // the range control is still shown
    expect(screen.getByLabelText("Custom")).toBeChecked();
  });

  it("groups by week when the bars would be under 4 px, and says so", async () => {
    mock((u) => (u.pathname === "/v1/workspaces/w1/usage" ? json(200, workspaceBody(u.searchParams.get("from")!, u.searchParams.get("to")!)) : null));
    window.history.replaceState(null, "", "/x?from=2026-01-01&to=2026-11-17");
    renderLevel(WS);
    expect(await screen.findByText("Shown by week")).toBeInTheDocument();
    expect(screen.getByText("Tokens per week")).toBeInTheDocument();
    expect(within(screen.getByTestId("bar-chart")).getAllByRole("listitem").length).toBeLessThan(60);
  });

  it("Show as table switches the chart to a table of the same days", async () => {
    mock((u) => (u.pathname === "/v1/workspaces/w1/usage" ? json(200, workspaceBody(u.searchParams.get("from")!, u.searchParams.get("to")!)) : null));
    renderLevel(WS);
    await screen.findByText("Tokens per day");
    fireEvent.click(screen.getByRole("button", { name: "Show as table" }));
    const table = screen.getByRole("table", { name: "Tokens per day" });
    expect(within(table).getAllByRole("row")).toHaveLength(31);
    fireEvent.click(screen.getByLabelText("Awake time"));
    expect(screen.getByRole("heading", { name: "Awake time per day" })).toBeInTheDocument();
  });

  it("has an empty state that keeps the range control", async () => {
    mock((u) =>
      u.pathname === "/v1/workspaces/w1/usage"
        ? json(200, { from: u.searchParams.get("from"), to: u.searchParams.get("to"), totals: z, by_day: [], by_agent: [], by_model: [], runtime_as_of: null })
        : null,
    );
    renderLevel(WS);
    expect(await screen.findByText("No usage in these days.")).toBeInTheDocument();
    expect(screen.getByLabelText("90 days")).toBeInTheDocument();
  });

  it("shows the shared error state when the request fails", async () => {
    mock((u) => (u.pathname === "/v1/workspaces/w1/usage" ? json(404, { error: { code: "not_found", message: "Not found." } }) : null));
    renderLevel(WS);
    await waitFor(() => expect(screen.queryByText("Loading…")).toBeNull());
    expect(screen.queryByText("Tokens per day")).toBeNull();
  });

  it("filtered to one agent (?agent=): the heading names the agent, a chip with a clear button replaces the picker, no By agent table", async () => {
    const calls = mock((u) =>
      u.pathname === "/v1/workspaces/w1/usage" ? json(200, workspaceBody(u.searchParams.get("from")!, u.searchParams.get("to")!, { by_agent: [{ agent_id: "a1", agent_name: "Scout", ...TOTALS }] })) : null,
    );
    window.history.replaceState(null, "", "/x?tab=usage&agent=a1");
    renderLevel(WS);
    expect(await screen.findByRole("heading", { name: "Usage of Scout" })).toBeInTheDocument();
    expect(calls.paths.some((p) => p.includes("agent_id=a1"))).toBe(true);
    expect(screen.queryByRole("heading", { name: "By agent" })).toBeNull();
    expect(screen.queryByLabelText("Agent")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: /Remove filter Agent: Scout/ }));
    expect(window.location.search).not.toContain("agent=");
    expect(window.location.search).toContain("tab=usage");
    expect(await screen.findByLabelText("Agent")).toBeInTheDocument();
  });
});

describe("UsageView in an organization and in Administration", () => {
  it("the organization view has a workspace filter, Top workspaces with Other workspaces, By model and no cost", async () => {
    const calls = mock((u) =>
      u.pathname === "/v1/orgs/o1/usage"
        ? json(200, {
            from: u.searchParams.get("from"),
            to: u.searchParams.get("to"),
            totals: TOTALS,
            by_day: days(u.searchParams.get("from")!, u.searchParams.get("to")!),
            by_workspace: [{ workspace_id: "w1", workspace_name: "Main", ...c({ input_tokens: 900_000 }) }, { workspace_id: "w9", workspace_name: null, ...c({ input_tokens: 5 }) }],
            other_workspaces: c({ input_tokens: 7 }),
            by_model: MODELS,
            runtime_as_of: null,
          })
        : null,
    );
    renderLevel({ kind: "org", orgId: "o1", orgSlug: "acme" });
    await screen.findByRole("heading", { name: "Top workspaces" });
    expect(screen.getByLabelText("Workspace")).toBeInTheDocument();
    expect(await screen.findByRole("link", { name: "Main" })).toHaveAttribute("href", "/o/acme/w/main/activity?tab=usage");
    expect(screen.getByText("Deleted workspace")).toBeInTheDocument();
    expect(screen.getByText("Other workspaces")).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "By agent" })).toBeNull();
    expect(document.body.textContent ?? "").not.toMatch(/\$|cost|USD/i);
    fireEvent.change(screen.getByLabelText("Workspace"), { target: { value: "w1" } });
    expect(window.location.search).toContain("workspace=w1");
    await waitFor(() => expect(calls.paths.some((p) => p.includes("workspace_id=w1"))).toBe(true));
  });

  it("Administration has an organization filter, By organization, Top workspaces and By model", async () => {
    const calls = mock((u) =>
      u.pathname === "/v1/admin/usage"
        ? json(200, {
            from: u.searchParams.get("from"),
            to: u.searchParams.get("to"),
            totals: TOTALS,
            by_day: days(u.searchParams.get("from")!, u.searchParams.get("to")!),
            by_org: [{ org_id: "o1", org_name: "Acme", ...c({ input_tokens: 900_000 }) }, { org_id: "o2", org_name: null, ...c({ input_tokens: 4 }) }],
            by_workspace: [{ workspace_id: "w1", workspace_name: "Main", org_id: "o1", org_name: "Acme", ...c({ input_tokens: 900_000 }) }],
            other_workspaces: null,
            by_model: MODELS,
            runtime_as_of: null,
          })
        : null,
    );
    renderLevel({ kind: "admin" });
    await screen.findByRole("heading", { name: "By organization" });
    expect(screen.getByRole("heading", { name: "Top workspaces" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "By model" })).toBeInTheDocument();
    expect(screen.getByText("Deleted organization")).toBeInTheDocument();
    expect(screen.queryByText("Other workspaces")).toBeNull();
    // an organization's name opens the view filtered to it
    fireEvent.click(screen.getByRole("button", { name: "Acme" }));
    expect(window.location.search).toContain("org=o1");
    await waitFor(() => expect(calls.paths.some((p) => p.includes("org_id=o1"))).toBe(true));
  });
});

describe("UsageCard on the Computer page", () => {
  it("shows the last 30 days of the agent and links to Activity › Usage filtered to it", async () => {
    const calls = mock((u) => (u.pathname === "/v1/workspaces/w1/usage" ? json(200, workspaceBody(u.searchParams.get("from")!, u.searchParams.get("to")!)) : null));
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={qc}>
        <UsageCard wsId="w1" org="acme" ws="main" agentId="a1" />
      </QueryClientProvider>,
    );
    await screen.findByText("1.2 M");
    expect(calls.paths.some((p) => p.includes("agent_id=a1"))).toBe(true);
    expect(screen.getByText("Turn time")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "See usage" })).toHaveAttribute("href", "/o/acme/w/main/activity?tab=usage&agent=a1");
  });
});
