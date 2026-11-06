import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { PeersTab } from "@/components/peers/peers-tab";
import { SchedulesTab } from "@/components/schedules/schedules-tab";
import { WebhooksTab } from "@/components/webhooks/webhooks-tab";

const json = (status: number, body: unknown) =>
  new Response(body === null ? null : JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const err = (status: number, code: string, message = code, details?: unknown) => json(status, { error: { code, message, details } });
type Handler = (req: Request) => Response | Promise<Response>;
function mockFetch(routes: Record<string, Handler>) {
  const fn = vi.fn(async (input: Request | string | URL) => {
    const req = input as Request;
    const url = new URL(req.url, "http://localhost");
    const h = routes[`${req.method} ${decodeURIComponent(url.pathname)}`];
    return h ? h(req) : err(404, "not_found", "nf");
  });
  vi.stubGlobal("fetch", fn);
  return fn;
}
let qcRef: QueryClient;
function wrap(ui: React.ReactNode) {
  qcRef = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qcRef}>{ui}</QueryClientProvider>);
}
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const sched = {
  id: "s1", agent_id: "a1", consecutive_failures: 0, created_at: "2026-01-01T00:00:00Z", cron: "0 9 * * *", enabled: true,
  name: "Morning", prompt: "Say hi", timezone: "Europe/Zurich", updated_at: "2026-01-01T00:00:00Z",
};

describe("SchedulesTab", () => {
  it("lists schedules, shows run outcomes and maps schedule_busy on Run now", async () => {
    mockFetch({
      "GET /v1/agents/a1/schedules": () => json(200, { items: [sched], next_cursor: null }),
      "GET /v1/schedules/s1/runs": () =>
        json(200, { items: [{ id: "r1", schedule_id: "s1", fired_at: "2026-01-02T09:00:00Z", scheduled_for: "2026-01-02T09:00:00Z", created_at: "2026-01-02T09:00:00Z", outcome: "gated_quiet" }], next_cursor: null }),
      "POST /v1/schedules/s1/run-now": () => err(409, "schedule_busy", "busy"),
    });
    wrap(<SchedulesTab agentId="a1" />);
    expect(await screen.findByText("Morning")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Runs" }));
    expect(await screen.findByText("gated_quiet")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Run now" }));
    expect((await screen.findByRole("alert")).textContent).toContain("already has a run in progress");
  });

  it("creates with the workspace timezone default and shows server cron validation", async () => {
    let body: Record<string, unknown> | null = null;
    let attempt = 0;
    mockFetch({
      "GET /v1/agents/a1/schedules": () => json(200, { items: [], next_cursor: null }),
      "POST /v1/agents/a1/schedules": async (r) => {
        body = (await r.json()) as Record<string, unknown>;
        attempt++;
        return attempt === 1
          ? err(422, "invalid_request", "bad", { fields: [{ path: "/cron", message: "Minimum interval is 5 minutes" }] })
          : json(201, sched);
      },
    });
    wrap(<SchedulesTab agentId="a1" defaultTimezone="Europe/Zurich" />);
    fireEvent.click(await screen.findByRole("button", { name: "New schedule" }));
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Fast" } });
    fireEvent.change(screen.getByLabelText("Prompt"), { target: { value: "go" } });
    fireEvent.change(screen.getByLabelText(/^Cron/), { target: { value: "* * * *" } });
    fireEvent.click(screen.getByRole("button", { name: "Create schedule" }));
    expect((await screen.findByRole("alert")).textContent).toContain("exactly 5 fields");
    expect(body).toBeNull();
    fireEvent.change(screen.getByLabelText(/^Cron/), { target: { value: "* * * * *" } });
    fireEvent.click(screen.getByRole("button", { name: "Create schedule" }));
    expect(await screen.findByText("Minimum interval is 5 minutes")).toBeTruthy();
    expect(body).toMatchObject({ name: "Fast", cron: "* * * * *", timezone: "Europe/Zurich" });
    fireEvent.change(screen.getByLabelText(/^Cron/), { target: { value: "*/10 * * * *" } });
    fireEvent.click(screen.getByRole("button", { name: "Create schedule" }));
    await waitFor(() => expect(screen.queryByRole("form", { name: "New schedule" })).toBeNull());
  });

  it("shows the forbidden state on 403", async () => {
    mockFetch({ "GET /v1/agents/a1/schedules": () => err(403, "forbidden", "no") });
    wrap(<SchedulesTab agentId="a1" />);
    expect((await screen.findByRole("alert")).textContent).toContain("do not have permission");
  });
});

const hook = {
  id: "w1", agent_id: "a1", created_at: "2026-01-01T00:00:00Z", enabled: true, instructions: "Handle it", name: "GitHub",
  updated_at: "2026-01-01T00:00:00Z", url_prefix: "whk_ab12", verification: { kind: "none" },
};

describe("WebhooksTab", () => {
  it("shows the URL once on create and drops it after closing; never in the query cache", async () => {
    const url = "https://aurora.example/hooks/whk_SECRETTOKEN";
    mockFetch({
      "GET /v1/agents/a1/webhooks": () => json(200, { items: [hook], next_cursor: null }),
      "POST /v1/agents/a1/webhooks": () => json(201, { url, webhook: hook }),
    });
    wrap(<WebhooksTab agentId="a1" />);
    fireEvent.click(await screen.findByRole("button", { name: "New webhook" }));
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "GitHub" } });
    fireEvent.change(screen.getByLabelText("Instructions"), { target: { value: "Handle it" } });
    fireEvent.click(screen.getByRole("button", { name: "Create webhook" }));
    expect((await screen.findByTestId("secret-key")).textContent).toBe(url);
    expect(screen.getByText(/You won't see this again/)).toBeTruthy();
    expect(JSON.stringify(qcRef.getQueryCache().getAll().map((q) => q.state.data))).not.toContain("SECRETTOKEN");
    fireEvent.click(screen.getByRole("button", { name: "I have saved it" }));
    await waitFor(() => expect(screen.queryByTestId("secret-key")).toBeNull());
    expect(document.body.textContent).not.toContain("SECRETTOKEN");
  });

  it("rotate requires confirmation and shows the new URL once", async () => {
    mockFetch({
      "GET /v1/agents/a1/webhooks": () => json(200, { items: [hook], next_cursor: null }),
      "POST /v1/webhooks/w1/rotate": () => json(200, { url: "https://x/whk_NEW", webhook: hook }),
    });
    wrap(<WebhooksTab agentId="a1" />);
    fireEvent.click(await screen.findByRole("button", { name: "Rotate URL" }));
    fireEvent.click(screen.getByRole("button", { name: "Confirm rotate GitHub" }));
    expect((await screen.findByTestId("secret-key")).textContent).toContain("whk_NEW");
    fireEvent.click(screen.getByRole("button", { name: "I have saved it" }));
    await waitFor(() => expect(screen.queryByTestId("secret-key")).toBeNull());
  });

  it("shows forbidden on 403/404", async () => {
    mockFetch({ "GET /v1/agents/a1/webhooks": () => err(404, "not_found", "nf") });
    wrap(<WebhooksTab agentId="a1" />);
    expect(await screen.findByRole("alert")).toBeTruthy();
  });
});

const link = {
  id: "l1", revision: 3, created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z",
  self: { agent_id: "a1", policy: "ask" },
  peer: { agent_id: "a2", handle: "bob", name: "Bob", workspace_id: "w2", policy: "allow" },
};

describe("PeersTab", () => {
  it("lists links and tasks; policy change sends revision; 409 refetches", async () => {
    let body: unknown = null;
    let linkFetches = 0;
    mockFetch({
      "GET /v1/agents/a1/links": () => {
        linkFetches++;
        return json(200, { items: [link] });
      },
      "GET /v1/agents/a1/peer-tasks": () =>
        json(200, {
          items: [{
            id: "t1", link_id: "l1", from_agent_id: "a1", to_agent_id: "a2", direction: "outgoing", origin_conversation_id: "c1",
            task_conversation_id: "c2", status: "completed", messages_used: 2, message_budget: 10, report: "All done",
            expires_at: "2026-02-01T00:00:00Z", created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z",
          }],
          next_cursor: null,
        }),
      "PUT /v1/agent-links/l1/policy": async (r) => {
        body = await r.json();
        return err(409, "stale_revision", "stale");
      },
    });
    wrap(<PeersTab agentId="a1" />);
    expect(await screen.findByText("Bob")).toBeTruthy();
    expect(await screen.findByText("All done")).toBeTruthy();
    expect(screen.getByText("Budget 2/10")).toBeTruthy();
    fireEvent.change(screen.getByLabelText("My policy for Bob"), { target: { value: "blocked" } });
    await waitFor(() => expect(body).toEqual({ agent_id: "a1", policy: "blocked", revision: 3 }));
    expect((await screen.findByRole("alert")).textContent).toContain("changed by someone else");
    await waitFor(() => expect(linkFetches).toBe(2));
  });
});
