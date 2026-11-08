import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ChatPane, ChatTab } from "@/components/agents/chat-tab";

// D-102 / P2-UI-5: the messaging layout. Behaviours that existed before (RFE-009/010/012/015, follow-ups, Stop,
// the live line, door chips, `no_model_key`) are covered here against the new markup; the pure grouping,
// ordering and summary helpers are in chat-layout.test.ts.

const st = vi.hoisted(() => ({
  events: [] as unknown[],
  deltas: {} as Record<string, unknown>,
  activity: {} as Record<string, string>,
  purged: {} as Record<string, string>,
  live: true,
}));
vi.mock("@/lib/chat/use-transcript", () => ({
  useTranscript: () => ({
    state: { events: st.events, deltas: st.deltas, activity: st.activity, purged: st.purged, lastSeq: st.events.length },
    loading: false,
    error: null,
    live: st.live,
  }),
}));

const json = (status: number, body: unknown) =>
  new Response(body === null ? null : JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const conv = (id: string, kind: string, over: Record<string, unknown> = {}) => ({
  id, agent_id: "a1", kind, external_ref: id, title: null, direct_user_id: null, direct_user: null, origin_conversation_id: null,
  last_seq: 0, app_turn_streak: 0, app_turn_capped: false, active_turn: null, last_activity_at: null, archived_at: null,
  created_at: "2026-01-01T00:00:00Z", ...over,
});
interface Call { method: string; path: string; body: unknown }
function mock(routes: Record<string, () => Response> = {}) {
  const calls: Call[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"), init);
      const url = new URL(req.url, "http://localhost");
      const text = req.method === "GET" || req.method === "DELETE" ? "" : await req.clone().text();
      calls.push({ method: req.method, path: url.pathname, body: text ? JSON.parse(text) : undefined });
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
const pane = (props: Record<string, unknown> = {}) =>
  wrap(<ChatPane agentId="a1" wsId="w" base="/o/x/w/y" convId="c1" direct={null} agent={{ id: "a1", name: "Daneel", brief: "Triage the team's pull requests." }} {...props} />);

const e = (seq: number, type: string, payload: Record<string, unknown>, ts = "2026-11-08T12:00:00Z") => ({ seq, type, ts, payload });
const MINUTE = (m: number, s = 0) => `2026-11-08T12:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}Z`;

beforeEach(() => {
  st.events = [];
  st.deltas = {};
  st.activity = {};
  st.purged = {};
  st.live = true;
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  window.history.replaceState(null, "", "/");
});

describe("thread header", () => {
  it("shows the Live pill, then Reconnecting… when the stream drops", () => {
    mock();
    const { unmount } = pane();
    expect(screen.getByText("Live")).toBeTruthy();
    unmount();
    st.live = false;
    pane();
    expect(screen.getByText("Reconnecting…")).toBeTruthy();
    expect(screen.queryByText("Live")).toBeNull();
  });

  it("joins the kind, the archived date and the retention window into one meta line", () => {
    mock();
    pane({ conversation: conv("c1", "group", { title: "Team room", archived_at: "2026-02-01T10:00:00Z" }), retentionDays: 90, archived: true, onGone: vi.fn() });
    expect(screen.getByRole("heading", { name: "Team room" })).toBeTruthy();
    const meta = screen.getByText(/Deleted after 90 days without activity/).closest("p")!;
    expect(meta.textContent).toMatch(/Group/);
    expect(meta.textContent).toMatch(/Archived 1 Feb 2026/);
  });

  it("the back button is labelled and calls back", () => {
    mock();
    const onBack = vi.fn();
    pane({ onBack });
    fireEvent.click(screen.getByRole("button", { name: "Back to conversations" }));
    expect(onBack).toHaveBeenCalled();
  });
});

describe("empty conversation", () => {
  it("shows the avatar card with the agent's brief and focuses the composer", () => {
    mock();
    pane();
    expect(screen.getByRole("heading", { name: "Start a conversation with Daneel" })).toBeTruthy();
    expect(screen.getByText("Triage the team's pull requests.")).toBeTruthy();
    expect(document.activeElement).toBe(screen.getByLabelText("Message"));
    expect(screen.getByPlaceholderText("Message Daneel…")).toBeTruthy();
  });
});

describe("transcript", () => {
  it("is a polite log; groups by sender; shows the door chip and the day divider", () => {
    st.events = [
      e(1, "turn.queued", { turn_id: "t1", speaker_label: "Ann", door: "slack", text: "First" }, MINUTE(0)),
      e(2, "turn.queued", { turn_id: "t2", speaker_label: "Ann", door: "slack", text: "Second" }, MINUTE(2)),
      e(3, "text", { turn_id: "t2", message_id: "m1", text: "Hello **Ann**" }, MINUTE(3)),
    ];
    mock();
    pane();
    const log = screen.getByRole("log");
    expect(log.getAttribute("aria-live")).toBe("polite");
    expect(within(log).getAllByText("Ann", { selector: "span" })).toHaveLength(1); // one meta line for both bubbles
    expect(within(log).getByText("slack")).toBeTruthy();
    expect(within(log).getByText("Daneel")).toBeTruthy();
    expect(within(log).getByText("Ann", { selector: "strong" })).toBeTruthy(); // markdown, safe mode
    expect(within(log).getAllByRole("separator")).toHaveLength(1);
  });

  it("shows the live activity line in a status row while a turn runs", () => {
    st.events = [e(1, "turn.queued", { turn_id: "t1", speaker_label: "Ann", text: "go" })];
    st.activity = { t1: "Waiting for approval" };
    mock();
    pane();
    expect(screen.getByRole("status", { name: "" }).textContent).toContain("Waiting for approval");
  });

  it("folds tool calls into one row: open while the turn runs, collapsed after, DENY in the summary", () => {
    const calls = [
      e(1, "turn.queued", { turn_id: "t1", speaker_label: "Ann", text: "go" }, MINUTE(0)),
      e(2, "tool.started", { turn_id: "t1", call_id: "c1", name: "github", summary: "GET /repos/acme/web" }, MINUTE(0, 1)),
      e(3, "tool.finished", { turn_id: "t1", call_id: "c1", ok: true, summary: "200" }, MINUTE(0, 3)),
      e(4, "tool.started", { turn_id: "t1", call_id: "c2", name: "linear", summary: "POST /issues" }, MINUTE(0, 4)),
      e(5, "tool.finished", { turn_id: "t1", call_id: "c2", ok: false, summary: "blocked by policy" }, MINUTE(0, 5)),
    ];
    st.events = calls;
    mock();
    const { unmount } = pane();
    const details = () => document.querySelector("details") as HTMLDetailsElement;
    expect(screen.getByText("Used 2 tools · 3.0 s")).toBeTruthy();
    expect(details().open).toBe(true); // turn t1 has no turn.finished
    expect(within(details()).getByText("Deny")).toBeTruthy();
    unmount();
    st.events = [...calls, e(6, "turn.finished", { turn_id: "t1", status: "done" }, MINUTE(0, 6))];
    pane();
    expect(details().open).toBe(false);
  });

  it("maps no_model_key to a danger alert with the LLM keys link", () => {
    st.events = [e(1, "error", { turn_id: "t1", code: "no_model_key", message: "x" })];
    mock();
    pane();
    const alert = screen.getByRole("alert");
    expect(alert.textContent).toContain("no usable LLM key");
    expect(within(alert).getByRole("link", { name: "Open LLM keys" }).getAttribute("href")).toBe("/o/x/w/y/credentials");
  });

  it("renders an expired attachment as a muted chip, never a link (RFE-015, C-196)", () => {
    st.events = [e(1, "attachment", { turn_id: "t1", attachment_id: "gone", filename: "report.pdf", size: 10 })];
    st.purged = { gone: "retention" };
    mock();
    pane();
    const chip = screen.getByTestId("attachment-expired");
    expect(chip.textContent).toContain("report.pdf");
    expect(chip.textContent).toContain("Expired by retention");
    expect(screen.queryByRole("link", { name: "report.pdf" })).toBeNull();
  });
});

describe("message actions", () => {
  it("has a toolbar with Copy and More; Copy writes the text; hidden when the clipboard is missing", async () => {
    st.events = [e(1, "turn.queued", { turn_id: "t1", speaker_label: "Ann", text: "my message" })];
    const writeText = vi.fn(async () => undefined);
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText } });
    mock();
    const { unmount } = pane();
    expect(screen.getByRole("toolbar", { name: "Message actions" })).toBeTruthy();
    fireEvent.click(await screen.findByRole("button", { name: "Copy message" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("my message"));
    expect(await screen.findByRole("button", { name: "Copied" })).toBeTruthy();
    unmount();
    vi.stubGlobal("navigator", { ...navigator, clipboard: undefined });
    pane();
    expect(screen.queryByRole("button", { name: /Copy message|Copied/ })).toBeNull();
    expect(screen.getByRole("button", { name: "More message actions" })).toBeTruthy();
  });
});

describe("inline approval (RFE-012)", () => {
  const pending = e(2, "approval.pending", { turn_id: "t1", approval_id: "ap1", summary: "Merge pull request #42" });
  const approval = {
    id: "ap1", status: "pending", created_at: "2026-11-08T12:00:00Z", deadline: new Date(Date.now() + 125_000).toISOString(), host: "api.github.com",
    method: "PUT", path: "/repos/acme/web/pulls/42/merge", rule_name: "Ask before merging",
  };
  const routes = (role: string) => ({
    "GET /v1/workspaces/w": () => json(200, { my_role: role }),
    "GET /v1/workspaces/w/approvals": () => json(200, { items: [approval], next_cursor: null }),
  });

  it("shows HELD, the countdown, the request line and the rule, with Approve and Deny for an approver", async () => {
    st.events = [e(1, "turn.queued", { turn_id: "t1", text: "merge it" }), pending];
    mock(routes("approver"));
    pane();
    expect(await screen.findByText("PUT")).toBeTruthy();
    expect(screen.getByText("Held")).toBeTruthy();
    expect(screen.getByText(/Expires in 2:0\d/)).toBeTruthy();
    expect(screen.getByText("api.github.com/repos/acme/web/pulls/42/merge")).toBeTruthy();
    expect(screen.getByText(/Ask before merging/)).toBeTruthy();
    expect(await screen.findByRole("button", { name: "Approve" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Deny" })).toBeTruthy();
  });

  it("shows a plain member 'Waiting for an approver' and no buttons", async () => {
    st.events = [pending];
    mock(routes("member"));
    pane();
    expect(await screen.findByText("Waiting for an approver")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Approve" })).toBeNull();
  });

  it("collapses to one line with the decision chip once decided", () => {
    st.events = [pending, e(3, "approval.decided", { approval_id: "ap1", decision: "approve", decided_by_label: "Ann" })];
    mock(routes("approver"));
    pane();
    expect(screen.getByText("Approved by Ann")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Approve" })).toBeNull();
  });
});

describe("composer", () => {
  it("Enter sends and Shift+Enter does not; Send is disabled while empty", async () => {
    const calls = mock({ "POST /v1/conversations/c1/messages": () => json(202, { turn: { id: "t1" }, followup: null }) });
    pane();
    const box = screen.getByLabelText("Message") as HTMLTextAreaElement;
    expect((screen.getByRole("button", { name: "Send" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.change(box, { target: { value: "line one" } });
    fireEvent.keyDown(box, { key: "Enter", shiftKey: true });
    expect(calls.some((c) => c.path.endsWith("/messages"))).toBe(false);
    fireEvent.keyDown(box, { key: "Enter" });
    await waitFor(() => expect(calls.find((c) => c.path.endsWith("/messages"))?.body).toEqual({ text: "line one" }));
    await waitFor(() => expect(box.value).toBe(""));
  });

  it("queues a follow-up above the box with Send now, and Stop aborts the running turn", async () => {
    st.events = [e(1, "turn.queued", { turn_id: "t1", speaker_label: "Ann", text: "go" })];
    const followup = { id: "f1", parent_turn_id: "t1", status: "joining", text: "also check CI", created_at: "2026-11-08T12:01:00Z" };
    const calls = mock({
      "POST /v1/conversations/c1/messages": () => json(202, { turn: null, followup }),
      "POST /v1/turns/t1/send-now": () => json(200, {}),
      "POST /v1/turns/t1/abort": () => json(200, {}),
    });
    pane();
    const box = screen.getByLabelText("Message");
    expect(screen.getByPlaceholderText("Add a follow-up…")).toBeTruthy();
    fireEvent.change(box, { target: { value: "also check CI" } });
    fireEvent.click(screen.getByRole("button", { name: "Follow up" }));
    expect(await screen.findByText(/Queued follow-up: also check CI/)).toBeTruthy();
    expect(screen.getByText("Queued")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Send now" }));
    await waitFor(() => expect(calls.some((c) => c.path === "/v1/turns/t1/send-now")).toBe(true));
    fireEvent.click(screen.getByRole("button", { name: "Stop" }));
    await waitFor(() => expect(calls.some((c) => c.path === "/v1/turns/t1/abort")).toBe(true));
  });

  it("shows the read-only audited notice instead of the composer, with Stop for a running turn (RFE-009)", () => {
    st.events = [e(1, "turn.queued", { turn_id: "t1", speaker_label: "Ann", text: "go" })];
    mock();
    pane({ ownerView: "Ann" });
    expect(screen.getByText(/Read-only view/)).toBeTruthy();
    expect(screen.queryByLabelText("Message")).toBeNull();
    expect(screen.getByRole("button", { name: "Stop" })).toBeTruthy();
  });
});

describe("conversation list and the URL (RFE-009)", () => {
  const list = [
    conv("mine", "direct", { direct_user_id: "me" }),
    conv("g1", "group", { title: "Team room", last_activity_at: "2026-11-07T10:00:00Z" }),
    conv("g2", "webhook", { title: "Release notes", active_turn: { id: "t9" }, last_activity_at: "2026-11-08T10:00:00Z" }),
  ];
  const routes = () => ({
    "POST /v1/agents/a1/conversations/direct": () => json(200, list[0]),
    "GET /v1/agents/a1/conversations": () => json(200, { items: list, next_cursor: null }),
  });

  it("orders own direct chat first, shows Working, filters by title and keeps the selection in the URL", async () => {
    mock(routes());
    wrap(<ChatTab agentId="a1" wsId="w" base="/o/x/w/y" />);
    const nav = await screen.findByRole("navigation", { name: "Conversations" });
    await within(nav).findByText("Team room");
    const names = within(nav).getAllByRole("button").filter((b) => b.hasAttribute("aria-current") || /Direct chat|Team room|Release notes/.test(b.textContent ?? "")).map((b) => b.textContent ?? "");
    expect(names[0]).toMatch(/Direct chat/);
    expect(names[1]).toMatch(/Release notes/); // newest activity first
    expect(within(nav).getByText("Working")).toBeTruthy();
    expect(within(nav).getByText("Direct chat", { selector: "span.block" }).closest("button")!.getAttribute("aria-current")).toBe("true");

    fireEvent.change(within(nav).getByLabelText("Filter conversations"), { target: { value: "team" } });
    expect(within(nav).queryByText("Release notes")).toBeNull();

    fireEvent.click(within(nav).getByText("Team room"));
    expect(window.location.search).toBe("?c=g1");
    expect(within(nav).getByText("Team room").closest("button")!.getAttribute("aria-current")).toBe("true");
    expect(screen.getByRole("heading", { name: "Team room", level: 2 })).toBeTruthy();

    // Back returns to the previous selection (phone: the list).
    act(() => {
      window.history.back();
    });
    await waitFor(() => expect(window.location.search).toBe(""));
  });

  it("a deep link opens the conversation", async () => {
    window.history.replaceState(null, "", "/?c=g1");
    mock(routes());
    wrap(<ChatTab agentId="a1" wsId="w" base="/o/x/w/y" />);
    expect(await screen.findByRole("heading", { name: "Team room", level: 2 })).toBeTruthy();
  });

  it("the Active/Archived switch is a segmented tablist and lists archived conversations", async () => {
    const calls: string[] = [];
    mock({
      ...routes(),
      "GET /v1/agents/a1/conversations": () => {
        calls.push("list");
        return json(200, { items: [conv("old", "group", { title: "Old room", archived_at: "2026-02-01T00:00:00Z" })], next_cursor: null });
      },
    });
    wrap(<ChatTab agentId="a1" wsId="w" base="/o/x/w/y" />);
    fireEvent.click(await screen.findByRole("tab", { name: "Archived" }));
    expect(await screen.findByText("Old room")).toBeTruthy();
    fireEvent.click(screen.getByText("Old room"));
    expect((await screen.findByRole("heading", { name: "Old room", level: 2 })).closest("header")!.textContent).toMatch(/Archived 1 Feb 2026/);
    fireEvent.keyDown(screen.getByRole("button", { name: "Conversation actions" }), { key: "Enter" });
    expect(screen.getByRole("menuitem", { name: "Unarchive" })).toBeTruthy();
  });

  it("an empty archive says so", async () => {
    mock({ ...routes(), "GET /v1/agents/a1/conversations": () => json(200, { items: [], next_cursor: null }) });
    wrap(<ChatTab agentId="a1" wsId="w" base="/o/x/w/y" />);
    fireEvent.click(await screen.findByRole("tab", { name: "Archived" }));
    expect(await screen.findByText("No archived conversations.")).toBeTruthy();
  });
});
