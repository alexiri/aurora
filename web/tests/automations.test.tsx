import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { OwnerLabel, ReassignPanel } from "@/components/automations/owner-controls";
import { MembersView } from "@/components/org/members-view";
import { SchedulesTab } from "@/components/schedules/schedules-tab";

const json = (status: number, body: unknown) =>
  new Response(body === null ? null : JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });

type Call = { method: string; path: string; search: string; body: unknown };

function mockFetch(routes: Record<string, () => Response>) {
  const calls: Call[] = [];
  const fn = vi.fn(async (input: Request | string | URL) => {
    const req = input instanceof Request ? input : null;
    const url = new URL(req ? req.url : String(input), "http://localhost");
    const method = req?.method ?? "GET";
    const text = req && method !== "GET" && method !== "DELETE" ? await req.clone().text() : "";
    calls.push({ method, path: url.pathname, search: url.search, body: text ? JSON.parse(text) : null });
    const h = routes[`${method} ${url.pathname}`];
    return h ? h() : json(404, { error: { code: "not_found", message: "nf" } });
  });
  vi.stubGlobal("fetch", fn);
  return calls;
}

function wrap(ui: ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const sched = (over: Record<string, unknown> = {}) => ({
  id: "s1",
  agent_id: "a1",
  name: "Daily digest",
  prompt: "p",
  cron: "0 9 * * *",
  timezone: "UTC",
  gate_command: null,
  gate_description: null,
  enabled: true,
  disabled_reason: null,
  consecutive_failures: 0,
  next_fire_at: null,
  last_fired_at: null,
  created_by: { id: "u1", display_name: "Alice" },
  anchor: "creator",
  origin_conversation_id: null,
  conversation_hidden: false,
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
  ...over,
});

describe("owner label", () => {
  it("shows a person, or Workspace for anchored items", () => {
    render(<OwnerLabel anchor="creator" creator={{ id: "u1", display_name: "Alice" }} />);
    expect(screen.getByText(/Owner: Alice/)).toBeInTheDocument();
    cleanup();
    render(<OwnerLabel anchor="workspace" creator={{ id: "u1", display_name: "Alice" }} />);
    expect(screen.getByText("Workspace")).toBeInTheDocument();
    expect(screen.queryByText(/Alice/)).not.toBeInTheDocument();
  });
});

describe("schedules tab", () => {
  const list = () => json(200, { items: [sched({ disabled_reason: "authorization", enabled: false })], next_cursor: null });

  it("members cannot reassign or anchor", async () => {
    mockFetch({ "GET /v1/agents/a1/schedules": list });
    wrap(<SchedulesTab agentId="a1" wsId="w1" canOwn={false} />);
    await screen.findByText("Daily digest");
    expect(screen.queryByRole("button", { name: /Reassign/ })).not.toBeInTheDocument();
    expect(screen.queryByLabelText(/Anchor .* to workspace/)).not.toBeInTheDocument();
  });

  it("owners reassign with re-enable and can anchor", async () => {
    const calls = mockFetch({
      "GET /v1/agents/a1/schedules": list,
      "GET /v1/workspaces/w1/access": () =>
        json(200, {
          items: [
            { user_id: "u1", display_name: "Alice", email: "a@x.io", implicit: false, role: "member" },
            { user_id: "u2", display_name: "Bob", email: "b@x.io", implicit: false, role: "member" },
          ],
          next_cursor: null,
        }),
      "PUT /v1/schedules/s1/creator": () => json(200, sched({ created_by: { id: "u2", display_name: "Bob" } })),
      "PATCH /v1/schedules/s1": () => json(200, sched({ anchor: "workspace" })),
    });
    wrap(<SchedulesTab agentId="a1" wsId="w1" canOwn />);
    await screen.findByText("Daily digest");
    expect(screen.getByText(/Disabled because its owner lost access/)).toBeInTheDocument();
    fireEvent.click(screen.getByLabelText("Anchor Daily digest to workspace"));
    await waitFor(() =>
      expect(calls.find((c) => c.method === "PATCH")?.body).toEqual({ anchor: "workspace" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Reassign…" }));
    const select = await screen.findByLabelText("New owner");
    // The current owner is not offered.
    await screen.findByRole("option", { name: /Bob/ });
    expect(screen.queryByRole("option", { name: /Alice/ })).not.toBeInTheDocument();
    fireEvent.change(select, { target: { value: "u2" } });
    expect(screen.getByLabelText(/Re-enable it/)).toBeChecked();
    fireEvent.click(screen.getByRole("button", { name: "Reassign" }));
    await waitFor(() =>
      expect(calls.find((c) => c.method === "PUT")?.body).toEqual({ user_id: "u2", re_enable: true }),
    );
  });
});

describe("reassign panel", () => {
  it("surfaces new_creator_unauthorized", async () => {
    mockFetch({
      "GET /v1/workspaces/w1/access": () =>
        json(200, { items: [{ user_id: "u2", display_name: "Bob", email: "b@x.io", implicit: false, role: "member" }], next_cursor: null }),
      "PUT /v1/webhooks/h1/creator": () =>
        json(422, { error: { code: "new_creator_unauthorized", message: "must be able to use the workspace" } }),
    });
    wrap(<ReassignPanel agentId="a1" wsId="w1" kind="webhook" id="h1" name="hook" onClose={() => {}} />);
    await screen.findByRole("option", { name: /Bob/ });
    fireEvent.change(screen.getByLabelText("New owner"), { target: { value: "u2" } });
    fireEvent.click(screen.getByRole("button", { name: "Reassign" }));
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent(/use the workspace/));
  });
});

describe("members offboarding", () => {
  const members = {
    items: [
      { user_id: "u1", email: "alice@x.io", display_name: "Alice", role: "member", status: "active", created_at: "2026-01-01T00:00:00Z" },
      { user_id: "u2", email: "bob@x.io", display_name: "Bob", role: "member", status: "active", created_at: "2026-01-01T00:00:00Z" },
    ],
    next_cursor: null,
  };
  const item = (id: string, name: string) => ({
    id, name, agent: { id: "a1", name: "Agent" }, workspace: { id: "w1", name: "Main" },
    enabled: true, anchor: "creator", next_fire_at: null, would_disable: true,
  });
  const offboarding = () =>
    json(200, { schedules: [item("s1", "Daily digest")], webhooks: [item("h1", "Deploy hook")], peer_tasks: [], api_keys: 2 });

  it("lists what removal disables and sends reassign_to with the removal", async () => {
    const calls = mockFetch({
      "GET /v1/orgs/o1/members": () => json(200, members),
      "GET /v1/orgs/o1/members/u1/offboarding": offboarding,
      "DELETE /v1/orgs/o1/members/u1": () => json(204, null),
    });
    wrap(<MembersView orgId="o1" viewer="owner" />);
    fireEvent.click(await screen.findByRole("button", { name: "Remove alice@x.io" }));
    expect(await screen.findByText(/Removing Alice disables these 2 schedules and webhooks/)).toBeInTheDocument();
    expect(screen.getByText("Daily digest")).toBeInTheDocument();
    expect(screen.getByText("Deploy hook")).toBeInTheDocument();
    expect(screen.getByText(/2 API key/)).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Reassign to"), { target: { value: "u2" } });
    fireEvent.click(screen.getByRole("button", { name: "Remove member" }));
    await waitFor(() => expect(calls.some((c) => c.method === "DELETE")).toBe(true));
    expect(calls.find((c) => c.method === "DELETE")?.search).toBe("?reassign_to=u2");
  });

  it("suspending asks first and sends reassign_to: workspace in the PATCH", async () => {
    const calls = mockFetch({
      "GET /v1/orgs/o1/members": () => json(200, members),
      "GET /v1/orgs/o1/members/u1/offboarding": offboarding,
      "PATCH /v1/orgs/o1/members/u1": () => json(200, members.items[0]),
    });
    wrap(<MembersView orgId="o1" viewer="owner" />);
    fireEvent.click(await screen.findByRole("button", { name: "Suspend alice@x.io" }));
    await screen.findByText(/Removing Alice disables/);
    expect(calls.some((c) => c.method === "PATCH")).toBe(false);
    fireEvent.change(screen.getByLabelText("Reassign to"), { target: { value: "workspace" } });
    fireEvent.click(screen.getByRole("button", { name: "Suspend member" }));
    await waitFor(() => expect(calls.find((c) => c.method === "PATCH")?.body).toEqual({ status: "suspended", reassign_to: "workspace" }));
  });

  it("shows the server refusal and keeps the dialog open", async () => {
    mockFetch({
      "GET /v1/orgs/o1/members": () => json(200, members),
      "GET /v1/orgs/o1/members/u1/offboarding": offboarding,
      "DELETE /v1/orgs/o1/members/u1": () =>
        json(422, { error: { code: "new_creator_unauthorized", message: "The new creator must be an active member who can use the workspace." } }),
    });
    wrap(<MembersView orgId="o1" viewer="owner" />);
    fireEvent.click(await screen.findByRole("button", { name: "Remove alice@x.io" }));
    await screen.findByText(/Removing Alice disables/);
    fireEvent.change(screen.getByLabelText("Reassign to"), { target: { value: "u2" } });
    fireEvent.click(screen.getByRole("button", { name: "Remove member" }));
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent(/can use the workspace/));
    expect(screen.getByRole("button", { name: "Remove member" })).toBeInTheDocument();
  });
});
