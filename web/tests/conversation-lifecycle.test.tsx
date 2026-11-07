import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ChatPane, ChatTab } from "@/components/agents/chat-tab";
import { OrgSettingsView } from "@/components/org/settings-view";
import type { Org } from "@/lib/api/queries";
import { applyEvent, emptyTranscript } from "@/lib/chat/transcript";

vi.mock("next/navigation", () => ({ useRouter: () => ({ replace: vi.fn() }) }));

const events = vi.hoisted(() => ({ list: [] as unknown[] }));
vi.mock("@/lib/chat/use-transcript", () => ({
  useTranscript: () => ({
    state: { events: events.list, deltas: {}, activity: {}, lastSeq: 3 },
    loading: false,
    error: null,
    live: true,
  }),
}));

const json = (status: number, body: unknown) =>
  new Response(body === null ? null : JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const conv = (id: string, kind: string, over: Record<string, unknown> = {}) => ({
  id, agent_id: "a1", kind, external_ref: id, title: null, direct_user_id: null, direct_user: null, origin_conversation_id: null,
  last_seq: 0, app_turn_streak: 0, app_turn_capped: false, active_turn: null, last_activity_at: null, archived_at: null,
  created_at: "2026-01-01T00:00:00Z", ...over,
});

interface Call { method: string; path: string; search: string; body: unknown }
function mock(routes: Record<string, () => Response>) {
  const calls: Call[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"), init);
      const url = new URL(req.url, "http://localhost");
      const text = req.method === "GET" || req.method === "DELETE" ? "" : await req.clone().text();
      calls.push({ method: req.method, path: url.pathname, search: url.search, body: text ? JSON.parse(text) : undefined });
      const h = routes[`${req.method} ${url.pathname}`];
      return h ? h() : json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
  return calls;
}
function wrap(ui: ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  events.list = [];
});

const queued = (seq: number, text: string) => ({ seq, type: "turn.queued", ts: "t", payload: { turn_id: "t1", speaker_label: "Ann", door: "dashboard", text } });

describe("redaction (RFE-010)", () => {
  it("renders a redacted event as its marker and never as content", () => {
    events.list = [
      { seq: 1, type: "turn.queued", ts: "t", payload: { turn_id: "t1", redacted: true, redacted_by: "Ann", reason: "pasted a password" } },
    ];
    mock({});
    wrap(<ChatPane agentId="a1" wsId="w" base="/o/x/w/y" convId="c1" direct={null} />);
    expect(screen.getByText(/Content redacted by Ann \(pasted a password\)/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Redact message/ })).toBeNull();
  });

  it("warns that it is irreversible and posts the reason and the session reset", async () => {
    events.list = [queued(2, "my secret")];
    const calls = mock({ "POST /v1/conversations/c1/events/2/redact": () => json(200, { redacted_seqs: [2], session_reset: true }) });
    wrap(<ChatPane agentId="a1" wsId="w" base="/o/x/w/y" convId="c1" direct={null} />);
    fireEvent.click(screen.getByRole("button", { name: "Redact message 2" }));
    expect(await screen.findByText(/This is irreversible/)).toBeTruthy();
    fireEvent.change(screen.getByLabelText("Reason (optional)"), { target: { value: "password" } });
    fireEvent.click(screen.getByLabelText(/reset the agent/));
    fireEvent.click(screen.getByRole("button", { name: "Redact" }));
    await waitFor(() => expect(calls.some((c) => c.path.endsWith("/redact"))).toBe(true));
    expect(calls.find((c) => c.path.endsWith("/redact"))?.body).toEqual({ reason: "password", reset_session: true });
  });

  it("shows a refusal in words when the server says 403", async () => {
    events.list = [queued(2, "x")];
    mock({ "POST /v1/conversations/c1/events/2/redact": () => json(403, { error: { code: "forbidden", message: "no" } }) });
    wrap(<ChatPane agentId="a1" wsId="w" base="/o/x/w/y" convId="c1" direct={null} />);
    fireEvent.click(screen.getByRole("button", { name: "Redact message 2" }));
    fireEvent.click(await screen.findByRole("button", { name: "Redact" }));
    expect((await screen.findByRole("alert")).textContent).toMatch(/not allowed/);
  });

  it("a redacted copy replaces the stale event in the transcript state", () => {
    const s1 = applyEvent(emptyTranscript, queued(1, "secret") as never);
    const red = { seq: 1, type: "turn.queued", ts: "t", payload: { turn_id: "t1", redacted: true } };
    const s2 = applyEvent(s1, red as never);
    expect(s2.events).toHaveLength(1);
    expect(s2.events[0].payload.redacted).toBe(true);
    // a stale (unredacted) copy never overwrites a redacted one
    expect(applyEvent(s2, queued(1, "secret") as never).events[0].payload.redacted).toBe(true);
  });
});

describe("conversation actions", () => {
  it("delete needs the typed confirmation, explains what remains, and calls DELETE", async () => {
    const calls = mock({ "DELETE /v1/conversations/c1": () => json(204, null) });
    const gone = vi.fn();
    wrap(<ChatPane agentId="a1" wsId="w" base="/o/x/w/y" convId="c1" direct={null} onGone={gone} />);
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(await screen.findByText(/not deleted/)).toBeTruthy();
    const submit = screen.getByRole("button", { name: "Delete conversation" }) as HTMLButtonElement;
    expect(submit.disabled).toBe(true);
    fireEvent.change(screen.getByLabelText(/Type "delete"/), { target: { value: "delete" } });
    expect(submit.disabled).toBe(false);
    fireEvent.click(submit);
    await waitFor(() => expect(gone).toHaveBeenCalled());
    expect(calls.some((c) => c.method === "DELETE" && c.path === "/v1/conversations/c1")).toBe(true);
  });

  it("delete refused while a turn runs shows the reason", async () => {
    mock({ "DELETE /v1/conversations/c1": () => json(409, { error: { code: "conversation_active", message: "active" } }) });
    wrap(<ChatPane agentId="a1" wsId="w" base="/o/x/w/y" convId="c1" direct={null} onGone={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    fireEvent.change(await screen.findByLabelText(/Type "delete"/), { target: { value: "delete" } });
    fireEvent.click(screen.getByRole("button", { name: "Delete conversation" }));
    expect((await screen.findByRole("alert")).textContent).toMatch(/Stop it first/);
  });

  it("offers JSON and zip export links and Unarchive for an archived conversation", () => {
    mock({});
    wrap(<ChatPane agentId="a1" wsId="w" base="/o/x/w/y" convId="c1" direct={null} archived onGone={vi.fn()} />);
    expect(screen.getByRole("link", { name: "Export JSON" }).getAttribute("href")).toBe("/v1/conversations/c1/export");
    expect(screen.getByRole("link", { name: /zip/ }).getAttribute("href")).toBe("/v1/conversations/c1/export?attachments=zip");
    expect(screen.getByRole("button", { name: "Unarchive" })).toBeTruthy();
  });

  it("the Archived filter lists archived conversations and the banner states the retention window", async () => {
    const calls = mock({
      "POST /v1/agents/a1/conversations/direct": () => json(200, conv("mine", "direct", { direct_user_id: "me" })),
      "GET /v1/agents/a1/conversations": () => json(200, { items: [conv("old", "group", { title: "Old room", archived_at: "2026-02-01T00:00:00Z" })], next_cursor: null }),
      "GET /v1/orgs/o1/retention": () => json(200, { conversations_days: 90, conversations_days_max: null }),
    });
    wrap(<ChatTab agentId="a1" wsId="w" base="/o/x/w/y" orgId="o1" />);
    expect(await screen.findByText(/deletes conversations after 90 days/)).toBeTruthy();
    fireEvent.click(await screen.findByRole("button", { name: "Archived" }));
    expect(await screen.findByText("Old room")).toBeTruthy();
    await waitFor(() => expect(calls.some((c) => c.path === "/v1/agents/a1/conversations" && c.search.includes("archived=true"))).toBe(true));
  });
});

describe("org retention setting", () => {
  const org = { id: "o1", slug: "acme", name: "Acme", my_role: "admin", settings: { members_can_create_workspaces: true } } as unknown as Org;

  it("validates the window and saves it", async () => {
    const calls = mock({
      "GET /v1/orgs/o1/retention": () => json(200, { conversations_days: null, conversations_days_max: 365 }),
      "PUT /v1/orgs/o1/retention": () => json(200, { conversations_days: 30, conversations_days_max: 365 }),
    });
    wrap(<OrgSettingsView org={org} />);
    const input = await screen.findByLabelText("Days of inactivity");
    const save = screen.getByRole("button", { name: "Save retention" });
    fireEvent.change(input, { target: { value: "3" } });
    fireEvent.click(save);
    expect((await screen.findByRole("alert")).textContent).toMatch(/7 to 3650/);
    fireEvent.change(input, { target: { value: "400" } });
    fireEvent.click(save);
    expect((await screen.findByRole("alert")).textContent).toMatch(/at most 365/);
    expect(calls.some((c) => c.method === "PUT")).toBe(false);
    fireEvent.change(input, { target: { value: "30" } });
    fireEvent.click(save);
    expect(await screen.findByText("Retention saved.")).toBeTruthy();
    expect(calls.find((c) => c.method === "PUT")?.body).toEqual({ conversations_days: 30 });
  });

  it("an empty value clears the window", async () => {
    const calls = mock({
      "GET /v1/orgs/o1/retention": () => json(200, { conversations_days: 30, conversations_days_max: null }),
      "PUT /v1/orgs/o1/retention": () => json(200, { conversations_days: null, conversations_days_max: null }),
    });
    wrap(<OrgSettingsView org={org} />);
    const input = (await screen.findByLabelText("Days of inactivity")) as HTMLInputElement;
    await waitFor(() => expect(input.value).toBe("30"));
    fireEvent.change(input, { target: { value: "" } });
    fireEvent.click(screen.getByRole("button", { name: "Save retention" }));
    await waitFor(() => expect(calls.find((c) => c.method === "PUT")?.body).toEqual({ conversations_days: null }));
  });

  it("members do not get the retention form", () => {
    mock({});
    wrap(<OrgSettingsView org={{ ...org, my_role: "member" } as Org} />);
    expect(screen.queryByLabelText("Days of inactivity")).toBeNull();
  });
});
