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

describe("markdown on both sides (P2-UI-5b)", () => {
  const PERSON = "Line one\nline two\n\n**bold** and `inline`\n\n- GitHub\n- Linear\n\n```\nnpm run build && echo <ok>\n```";

  it("renders a person's message as safe markdown and keeps a single typed newline as a line break", () => {
    st.events = [e(1, "turn.queued", { turn_id: "t1", speaker_label: "Ann", text: PERSON })];
    mock();
    pane();
    const log = screen.getByRole("log");
    const p = within(log).getByText(/Line one/).closest("p")!;
    expect(p.querySelectorAll("br")).toHaveLength(1);
    expect(p.textContent).toBe("Line oneline two");
    expect(within(log).getByText("bold", { selector: "strong" })).toBeTruthy();
    expect(within(log).getByText("inline", { selector: "code" })).toBeTruthy();
    expect(within(log).getAllByRole("listitem").map((li) => li.textContent)).toEqual(["GitHub", "Linear"]);
    expect(log.querySelector("pre code")!.textContent).toBe("npm run build && echo <ok>");
  });

  it("shows <script> and <img onerror> in a person's message as inert text", () => {
    const evil = '<script>window.__pwn = 1</script>\n<img src=x onerror="window.__pwn = 2">';
    st.events = [e(1, "turn.queued", { turn_id: "t1", speaker_label: "Ann", text: evil })];
    mock();
    pane();
    const log = screen.getByRole("log");
    expect(log.querySelector("script, img")).toBeNull();
    expect(log.textContent).toContain("<script>window.__pwn = 1</script>");
    expect(log.textContent).toContain('<img src=x onerror="window.__pwn = 2">');
    expect((window as unknown as { __pwn?: number }).__pwn).toBeUndefined();
  });

  it("does not link a javascript: URL in a person's message", () => {
    st.events = [e(1, "turn.queued", { turn_id: "t1", speaker_label: "Ann", text: "[click](javascript:alert(1)) [ok](https://example.com/x)" })];
    mock();
    pane();
    const links = within(screen.getByRole("log")).getAllByRole("link");
    expect(links.map((a) => a.getAttribute("href"))).toEqual(["https://example.com/x"]);
  });

  it("leaves agent rendering unchanged: a single newline still joins the paragraph", () => {
    st.events = [e(1, "text", { turn_id: "t1", message_id: "m1", text: "Alpha\nbeta **bold**" })];
    mock();
    pane();
    const p = within(screen.getByRole("log")).getByText(/Alpha/).closest("p")!;
    expect(p.querySelectorAll("br")).toHaveLength(0);
    expect(p.textContent).toBe("Alpha beta bold");
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

  it("maps no_model_key to a danger alert with the LLM keys link and the reason counts", () => {
    st.events = [e(1, "error", { turn_id: "t1", code: "no_model_key", message: "No usable LLM key: 1 granted key not usable (origin_unverified)." })];
    mock();
    pane();
    const alert = screen.getByRole("alert");
    expect(alert.textContent).toContain("no usable LLM key");
    expect(alert.textContent).toContain("1 granted key not usable (origin_unverified)");
    expect(within(alert).getByRole("link", { name: "Open LLM keys" }).getAttribute("href")).toBe("/o/x/w/y/credentials?tab=llm");
    // Not an owner (the workspace did not load): no "Agent access" link.
    expect(within(alert).queryByRole("link", { name: "Agent access" })).toBeNull();
  });

  it("gives workspace owners the Agent access link next to Open LLM keys", async () => {
    st.events = [e(1, "error", { turn_id: "t1", code: "no_model_key", message: "No LLM key is granted to this agent." })];
    mock({ "GET /v1/workspaces/w": () => json(200, { id: "w", org_id: "o", my_role: "owner" }) });
    pane();
    const link = await screen.findByRole("link", { name: "Agent access" });
    expect(link.getAttribute("href")).toBe("?tab=access");
  });

  it("maps no_default_model to 'Choose a model' on the Instructions tab", () => {
    st.events = [e(1, "error", { turn_id: "t1", code: "no_default_model", message: "An LLM key is usable but no model is chosen." })];
    mock();
    pane();
    const alert = screen.getByRole("alert");
    expect(alert.textContent).toContain("This agent has an LLM key but no model is chosen.");
    expect(within(alert).getByRole("link", { name: "Choose a model" }).getAttribute("href")).toBe("?tab=instructions");
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

  // FIX-029 (C-615): a start-failure error block gains **Retry now**; after the 202 the failed
  // turn's text is re-posted as a new message (the failed turn stays failed).
  it("offers Retry now on a start-failure error and re-posts the turn text after the 202", async () => {
    st.events = [
      e(1, "turn.queued", { turn_id: "t1", speaker_label: "Ann", text: "summarise the log" }),
      e(2, "error", { turn_id: "t1", code: "pod_failed", message: "the pod crashed" }),
    ];
    const calls = mock({
      "GET /v1/agents/a1/sandbox": () =>
        json(200, {
          agent_id: "a1", state: "failed", boot_id: "b1", runner_id: null, image: null, started_at: null,
          keep_awake_reasons: [], failure_code: "pod_failed", failure_message: "the pod crashed", start_attempt: null,
          retryable: true, start_log: [], processes: [], ports: [], restart_to_update: false,
        }),
      "POST /v1/agents/a1/sandbox/retry": () =>
        json(202, {
          agent_id: "a1", state: "stopped", boot_id: null, runner_id: null, image: null, started_at: null,
          keep_awake_reasons: [], failure_code: null, failure_message: null, start_attempt: null,
          retryable: false, start_log: [], processes: [], ports: [], restart_to_update: false,
        }),
      "POST /v1/conversations/c1/messages": () => json(202, { turn: { id: "t2" }, followup: null }),
    });
    pane();
    const alert = screen.getByRole("alert");
    expect(alert.textContent).toContain("ended while it was starting");
    const btn = await screen.findByRole("button", { name: "Retry now" });
    fireEvent.click(btn);
    await waitFor(() => {
      const post = calls.find((c) => c.method === "POST" && c.path === "/v1/conversations/c1/messages");
      expect(post?.body).toEqual({ text: "summarise the log" });
    });
  });

  it("offers no Retry now for a non-start error (e.g. no_model_key)", () => {
    st.events = [e(1, "error", { turn_id: "t1", code: "no_model_key", message: "No usable LLM key." })];
    mock();
    pane();
    const alert = screen.getByRole("alert");
    expect(within(alert).queryByRole("button", { name: "Retry now" })).toBeNull();
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
  // The deadline is computed when the route is called, not when the file is collected: earlier
  // tests in this file can take seconds, which turned "2:05" into "1:59" on a busy machine.
  const approval = () => ({
    id: "ap1", status: "pending", created_at: "2026-11-08T12:00:00Z", deadline: new Date(Date.now() + 125_000).toISOString(), host: "api.github.com",
    method: "PUT", path: "/repos/acme/web/pulls/42/merge", rule_name: "Ask before merging",
  });
  const routes = (role: string) => ({
    "GET /v1/workspaces/w": () => json(200, { my_role: role }),
    "GET /v1/workspaces/w/approvals": () => json(200, { items: [approval()], next_cursor: null }),
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
