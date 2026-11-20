import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";

import { AccountsBulkBar } from "@/components/accounts/accounts-bulk";
import { AutomationsBulkBar } from "@/components/automations/automations-bulk";
import { MembersView } from "@/components/org/members-view";
import { AccessTab } from "@/components/settings/access-tab";
import { UndoToastHost } from "@/components/ui/undo-toast";
import { runBulk } from "@/lib/bulk";
import { json } from "./support/api";
import { seedMoreControls, wrap } from "./support/render";

// P5-REFINE-19 item 3: bulk actions are Expert; one confirm with the count, then the per-item routes in turn with a progress
// line; a failure is listed by name with Retry; nothing is rolled back; you cannot select yourself or the last owner.

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

type Call = { method: string; path: string; search: string; body: unknown };

/** Plain `fetch("/v1/me")` and the generated client's `Request`, both routed by `METHOD /path`. */
function serve(routes: Record<string, (call: Call) => Response | Promise<Response>>) {
  const calls: Call[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"), init);
      const url = new URL(req.url);
      const text = req.method !== "GET" && req.method !== "DELETE" ? await req.clone().text() : "";
      const call: Call = { method: req.method, path: url.pathname, search: url.search, body: text ? JSON.parse(text) : null };
      calls.push(call);
      const h = routes[`${req.method} ${url.pathname}`];
      return h ? h(call) : json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
  return calls;
}

describe("runBulk", () => {
  it("runs the items in order, reports progress, keeps failures by name and never throws", async () => {
    const order: string[] = [];
    const progress: string[] = [];
    const out = await runBulk(
      [{ id: "1", name: "Ann" }, { id: "2", name: "Bob" }, { id: "3", name: "Cy" }],
      async (t) => {
        order.push(t.id);
        if (t.id === "2") throw new Error("nope");
      },
      (d, t) => progress.push(`${d} of ${t}`),
    );
    expect(order).toEqual(["1", "2", "3"]);
    expect(progress).toEqual(["1 of 3", "2 of 3", "3 of 3"]);
    expect(out.done).toBe(2);
    expect(out.failed).toHaveLength(1);
    expect(out.failed[0]).toMatchObject({ id: "2", name: "Bob" });
  });
});

// ---- People of an organization -------------------------------------------------------------------------------------

const row = (id: string, name: string, role = "member", over: Record<string, unknown> = {}) => ({
  user_id: id, email: `${name.toLowerCase()}@x.io`, display_name: name, role, status: "active", created_at: "2026-01-01T00:00:00Z", ...over,
});
const members = { items: [row("u0", "Ada", "owner"), row("u1", "Alice"), row("u2", "Bob"), row("u3", "Cy")], next_cursor: null };

function orgRoutes(extra: Record<string, (c: Call) => Response | Promise<Response>> = {}) {
  return serve({
    "GET /v1/me": () => json(200, { user: { id: "u0", display_name: "Ada", email: "ada@x.io" }, memberships: [] }),
    "GET /v1/orgs/o1": () => json(200, { id: "o1", slug: "acme", name: "Acme", my_role: "owner", settings: {} }),
    "GET /v1/orgs/o1/members": () => json(200, members),
    "GET /v1/orgs/o1/invitations": () => json(200, { items: [], next_cursor: null }),
    "GET /v1/instance": () => json(200, { version: "0", features: {} }),
    ...extra,
  });
}

const tick = (name: string) => fireEvent.click(screen.getByRole("checkbox", { name: `Select ${name}` }));

describe("People of an organization", () => {
  it("has no checkboxes and no bar with Show more controls off", async () => {
    orgRoutes();
    wrap(<MembersView orgId="o1" viewer="owner" />, { moreControls: false });
    await screen.findByText("Alice");
    expect(screen.queryByRole("checkbox")).toBeNull();
    expect(screen.queryByTestId("bulk-bar")).toBeNull();
  });

  it("does not let you select yourself, and says why", async () => {
    orgRoutes();
    wrap(<MembersView orgId="o1" viewer="owner" />);
    await screen.findByText("Alice");
    const me = screen.getByRole("checkbox", { name: "Select Ada" });
    expect(me).toBeDisabled();
    expect(me).toHaveAttribute("title", "You cannot change yourself");
    expect(screen.getByRole("checkbox", { name: "Select Alice" })).toBeEnabled();
  });

  it("changes three people's role: one question with the count, three calls, 3 of 3", async () => {
    const calls = orgRoutes({ "PATCH /v1/orgs/o1/members/u1": () => json(200, {}), "PATCH /v1/orgs/o1/members/u2": () => json(200, {}), "PATCH /v1/orgs/o1/members/u3": () => json(200, {}) });
    wrap(<MembersView orgId="o1" viewer="owner" />);
    await screen.findByText("Alice");
    tick("Alice");
    tick("Bob");
    tick("Cy");
    const bar = screen.getByTestId("bulk-bar");
    expect(bar).toHaveTextContent("3 people selected");
    fireEvent.click(within(bar).getByRole("button", { name: "Change role…" }));
    const dialog = await screen.findByRole("dialog", { name: "Change the role of 3 people in Acme?" });
    fireEvent.change(within(dialog).getByLabelText("New role"), { target: { value: "admin" } });
    // What an Admin can do is said before anything is sent.
    expect(within(dialog).getByRole("list", { name: "What Admins can do" })).toBeInTheDocument();
    expect(calls.some((c) => c.method === "PATCH")).toBe(false);
    fireEvent.click(within(dialog).getByRole("button", { name: "Change role of 3 people" }));
    await waitFor(() => expect(within(bar).getByTestId("bulk-progress")).toHaveTextContent("3 of 3 done"));
    const patches = calls.filter((c) => c.method === "PATCH");
    expect(patches.map((c) => c.path)).toEqual(["/v1/orgs/o1/members/u1", "/v1/orgs/o1/members/u2", "/v1/orgs/o1/members/u3"]);
    expect(patches.every((c) => JSON.stringify(c.body) === JSON.stringify({ role: "admin" }))).toBe(true);
  });

  it("lists a forced failure by name with Retry, and retries only that one", async () => {
    let bobTries = 0;
    const calls = orgRoutes({
      "PATCH /v1/orgs/o1/members/u1": () => json(200, {}),
      "PATCH /v1/orgs/o1/members/u2": () => {
        bobTries++;
        return bobTries === 1 ? json(409, { error: { code: "managed_by_idp", message: "That role comes from your identity provider." } }) : json(200, {});
      },
      "PATCH /v1/orgs/o1/members/u3": () => json(200, {}),
    });
    wrap(<MembersView orgId="o1" viewer="owner" />);
    await screen.findByText("Alice");
    tick("Alice");
    tick("Bob");
    tick("Cy");
    fireEvent.click(screen.getByRole("button", { name: "Change role…" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("New role"), { target: { value: "admin" } });
    fireEvent.click(within(dialog).getByRole("button", { name: /^Change role of 3 people/ }));
    const alert = await screen.findByRole("alert");
    // The others went through and stay changed; Bob is named, with the server's words.
    expect(alert).toHaveTextContent("1 was not changed. The others were.");
    expect(alert).toHaveTextContent("Bob: ");
    expect(calls.filter((c) => c.method === "PATCH")).toHaveLength(3);
    fireEvent.click(within(alert).getByRole("button", { name: "Retry Bob" }));
    await waitFor(() => expect(bobTries).toBe(2));
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
    expect(calls.filter((c) => c.method === "PATCH" && c.path.endsWith("/u1"))).toHaveLength(1);
  });

  it("removes people after one question that says what happens", async () => {
    const calls = orgRoutes({ "DELETE /v1/orgs/o1/members/u1": () => json(204, null), "DELETE /v1/orgs/o1/members/u2": () => json(204, null) });
    wrap(<MembersView orgId="o1" viewer="owner" />);
    await screen.findByText("Alice");
    tick("Alice");
    tick("Bob");
    fireEvent.click(screen.getByRole("button", { name: "Remove…" }));
    const dialog = await screen.findByRole("dialog", { name: "Remove 2 people from Acme?" });
    expect(within(dialog).getByText("They lose access now.")).toBeInTheDocument();
    expect(calls.some((c) => c.method === "DELETE")).toBe(false);
    fireEvent.click(within(dialog).getByRole("button", { name: "Remove 2 people" }));
    await waitFor(() => expect(calls.filter((c) => c.method === "DELETE").map((c) => c.path)).toEqual(["/v1/orgs/o1/members/u1", "/v1/orgs/o1/members/u2"]));
  });

  it("select all ticks everyone who can be ticked, and not yourself", async () => {
    orgRoutes();
    wrap(<MembersView orgId="o1" viewer="owner" />);
    await screen.findByText("Alice");
    fireEvent.click(screen.getByRole("checkbox", { name: "Select all people" }));
    expect(screen.getByTestId("bulk-bar")).toHaveTextContent("3 people selected");
    expect(screen.getByRole("checkbox", { name: "Select Ada" })).not.toBeChecked();
  });
});

// ---- People of a workspace -----------------------------------------------------------------------------------------

describe("People of a workspace", () => {
  const bind = (id: string, name: string, role = "member", over: Record<string, unknown> = {}) => ({
    user_id: id, email: `${name.toLowerCase()}@x.io`, display_name: name, role, implicit: false, status: "active", joined_at: "2026-01-01T00:00:00Z", signed_in: true, ...over,
  });

  it("removes selected people from the workspace, never yourself, the inherited owner or the last owner", async () => {
    const calls = serve({
      "GET /v1/me": () => json(200, { user: { id: "u0", display_name: "Ada", email: "ada@x.io" }, memberships: [] }),
      "GET /v1/workspaces/w/access": () => json(200, { items: [bind("u0", "Ada", "owner"), bind("u1", "Alice"), bind("u2", "Bob"), bind("u9", "Boss", "owner", { implicit: true })], next_cursor: null }),
      "GET /v1/workspaces/w": () => json(200, { id: "w", org_id: "o", name: "Ops", my_role: "owner" }),
      "GET /v1/orgs/o": () => json(200, { id: "o", slug: "users", name: "Users", my_role: "admin" }),
      "GET /v1/orgs/o/members": () => json(200, { items: [], next_cursor: null }),
      "DELETE /v1/workspaces/w/access/u1": () => json(204, null),
      "DELETE /v1/workspaces/w/access/u2": () => json(204, null),
    });
    wrap(<AccessTab wsId="w" orgId="o" canOwn />);
    await screen.findByText("Alice");
    expect(screen.getByRole("checkbox", { name: "Select Ada" })).toBeDisabled();
    expect(screen.getByRole("checkbox", { name: "Select Boss" })).toBeDisabled();
    fireEvent.click(screen.getByRole("checkbox", { name: "Select all people" }));
    expect(screen.getByTestId("bulk-bar")).toHaveTextContent("2 people selected");
    fireEvent.click(screen.getByRole("button", { name: "Remove…" }));
    const dialog = await screen.findByRole("dialog", { name: "Remove 2 people from Ops?" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Remove 2 people" }));
    await waitFor(() => expect(calls.filter((c) => c.method === "DELETE")).toHaveLength(2));
  });

  it("has no checkboxes with Show more controls off, and none for a member who is not an owner", async () => {
    serve({
      "GET /v1/me": () => json(200, { user: { id: "u0", display_name: "Ada", email: "a@x.io" }, memberships: [] }),
      "GET /v1/workspaces/w/access": () => json(200, { items: [bind("u1", "Alice")], next_cursor: null }),
      "GET /v1/workspaces/w": () => json(200, { id: "w", org_id: "o", name: "Ops", my_role: "owner" }),
      "GET /v1/orgs/o": () => json(200, { id: "o", slug: "users", name: "Users", my_role: "admin" }),
      "GET /v1/orgs/o/members": () => json(200, { items: [], next_cursor: null }),
    });
    wrap(<AccessTab wsId="w" orgId="o" canOwn />, { moreControls: false });
    await screen.findByText("Alice");
    expect(screen.queryByRole("checkbox")).toBeNull();
    cleanup();
    wrap(<AccessTab wsId="w" orgId="o" canOwn={false} />);
    await screen.findByText("Alice");
    expect(screen.queryByRole("checkbox")).toBeNull();
  });
});

// ---- Accounts ------------------------------------------------------------------------------------------------------

const account = (id: string, name: string, over: Record<string, unknown> = {}) => ({
  ref: { type: "connection", id }, kind: "app", name, scope: "workspace", levels: ["read", "ask", "act"],
  health: { state: "ok" }, agents: [], can: { manage: true, give_access: true, change_access: true }, ...over,
});

function mountWith(ui: React.ReactNode, more = true) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  seedMoreControls(qc, more);
  return render(
    <QueryClientProvider client={qc}>
      {ui}
      <UndoToastHost />
    </QueryClientProvider>,
  );
}

describe("Accounts", () => {
  const picked = [account("c1", "Linear"), account("c2", "GitHub"), account("k1", "Anthropic key", { ref: { type: "secret", id: "k1" }, kind: "ai_key", levels: [] })] as never;

  it("gives the picked accounts to one agent at the lowest level, once confirmed", async () => {
    const calls = serve({
      "GET /v1/workspaces/w1/agents": () => json(200, { items: [{ id: "a1", name: "Daneel", handle: "daneel" }, { id: "a2", name: "Giskard", handle: "giskard" }], next_cursor: null }),
      "PUT /v1/agents/a2/grants/connections/c1": () => json(200, {}),
      "PUT /v1/agents/a2/grants/connections/c2": () => json(200, {}),
      "PUT /v1/agents/a2/grants/secrets/k1": () => json(200, {}),
    });
    mountWith(<AccountsBulkBar picked={picked} scope={{ kind: "workspace", id: "w1" }} orgId="o1" onClear={() => undefined} />);
    fireEvent.click(screen.getByRole("button", { name: "Give to an agent…" }));
    const dialog = await screen.findByRole("dialog");
    await waitFor(() => expect(within(dialog).getByLabelText("Agent")).toHaveDisplayValue("Daneel"));
    fireEvent.change(within(dialog).getByLabelText("Agent"), { target: { value: "a2" } });
    expect(calls.some((c) => c.method === "PUT")).toBe(false);
    fireEvent.click(within(dialog).getByRole("button", { name: "Give to Giskard" }));
    await waitFor(() => expect(screen.getByTestId("bulk-progress")).toHaveTextContent("3 of 3 done"));
    const puts = calls.filter((c) => c.method === "PUT");
    expect(puts.map((c) => c.path)).toEqual(["/v1/agents/a2/grants/connections/c1", "/v1/agents/a2/grants/connections/c2", "/v1/agents/a2/grants/secrets/k1"]);
    expect(puts[0]!.body).toEqual({ level: "read" });
  });

  it("turns off the accounts that have a switch after one question, and says which have none", async () => {
    const calls = serve({
      "GET /v1/workspaces/w1/agents": () => json(200, { items: [], next_cursor: null }),
      "PATCH /v1/connections/c1": () => json(200, {}),
      "PATCH /v1/connections/c2": () => json(200, {}),
    });
    mountWith(<AccountsBulkBar picked={picked} scope={{ kind: "workspace", id: "w1" }} orgId="o1" onClear={() => undefined} />);
    fireEvent.click(screen.getByRole("button", { name: "Turn off…" }));
    const dialog = await screen.findByRole("dialog", { name: "Turn off 2 accounts?" });
    expect(within(dialog).getByText(/Anthropic key \(Only an app account has a switch\)/)).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "Turn off 2 accounts" }));
    await waitFor(() => expect(calls.filter((c) => c.method === "PATCH")).toHaveLength(2));
    expect(calls.filter((c) => c.method === "PATCH").every((c) => JSON.stringify(c.body) === JSON.stringify({ status: "disabled" }))).toBe(true);
  });
});

// ---- Automations ---------------------------------------------------------------------------------------------------

describe("Automations", () => {
  const picked = [
    { id: "s1", name: "Morning", enabled: true },
    { id: "s2", name: "Evening", enabled: true },
    { id: "s3", name: "Weekly", enabled: false },
  ];

  it("Pause acts at once and a toast offers Undo, which resumes them", async () => {
    const calls = serve({ "PATCH /v1/schedules/s1": () => json(200, {}), "PATCH /v1/schedules/s2": () => json(200, {}), "GET /v1/agents/a1/schedules": () => json(200, { items: [], next_cursor: null }) });
    mountWith(<AutomationsBulkBar kind="schedule" agentId="a1" picked={picked} onClear={() => undefined} />);
    fireEvent.click(screen.getByRole("button", { name: "Pause" }));
    await waitFor(() => expect(screen.getByText("Paused 2 schedules")).toBeInTheDocument());
    expect(calls.filter((c) => c.method === "PATCH").map((c) => `${c.path} ${JSON.stringify(c.body)}`)).toEqual([
      `/v1/schedules/s1 ${JSON.stringify({ enabled: false })}`,
      `/v1/schedules/s2 ${JSON.stringify({ enabled: false })}`,
    ]);
    fireEvent.click(screen.getByRole("button", { name: "Undo" }));
    await waitFor(() => expect(calls.filter((c) => c.method === "PATCH")).toHaveLength(4));
    expect(calls.filter((c) => c.method === "PATCH").slice(2).every((c) => JSON.stringify(c.body) === JSON.stringify({ enabled: true }))).toBe(true);
  });

  it("Resume is for the paused ones, and Delete… asks for the count to be typed", async () => {
    const calls = serve({
      "PATCH /v1/schedules/s3": () => json(200, {}),
      "DELETE /v1/schedules/s1": () => json(204, null),
      "DELETE /v1/schedules/s2": () => json(204, null),
      "DELETE /v1/schedules/s3": () => json(204, null),
      "GET /v1/agents/a1/schedules": () => json(200, { items: [], next_cursor: null }),
    });
    mountWith(<AutomationsBulkBar kind="schedule" agentId="a1" picked={picked} onClear={() => undefined} />);
    fireEvent.click(screen.getByRole("button", { name: "Resume" }));
    await waitFor(() => expect(screen.getByText("Resumed 1 schedule")).toBeInTheDocument());
    expect(calls.find((c) => c.method === "PATCH")!.path).toBe("/v1/schedules/s3");
    fireEvent.click(screen.getByRole("button", { name: "Delete…" }));
    const dialog = await screen.findByRole("dialog", { name: "Delete 3 schedules?" });
    const confirm = within(dialog).getByRole("button", { name: "Delete 3 schedules" });
    expect(confirm).toBeDisabled();
    fireEvent.change(within(dialog).getByLabelText(/Type the number of schedules to confirm/), { target: { value: "3" } });
    expect(confirm).toBeEnabled();
    expect(calls.some((c) => c.method === "DELETE")).toBe(false);
    fireEvent.click(confirm);
    await waitFor(() => expect(calls.filter((c) => c.method === "DELETE")).toHaveLength(3));
  });

  it("is a bar only while something is selected, and not at all below Expert", () => {
    serve({});
    mountWith(<AutomationsBulkBar kind="webhook" agentId="a1" picked={[]} onClear={() => undefined} />);
    expect(screen.queryByTestId("bulk-bar")).toBeNull();
    cleanup();
    mountWith(<AutomationsBulkBar kind="webhook" agentId="a1" picked={picked} onClear={() => undefined} />, false);
    expect(screen.queryByTestId("bulk-bar")).toBeNull();
  });
});
