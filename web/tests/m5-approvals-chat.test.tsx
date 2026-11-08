import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ChatPane } from "@/components/agents/chat-tab";
import { TranscriptView } from "@/components/chat/transcript-view";
import { ActionApprovalCard, ApprovalsView } from "@/components/approvals/approvals-view";
import type { TranscriptEvent } from "@/lib/api/conversations";
import { applyEvents, emptyTranscript } from "@/lib/chat/transcript";

vi.mock("@/lib/chat/use-transcript", () => ({
  useTranscript: () => ({ state: { events: [], deltas: {}, activity: {}, lastSeq: 0 }, loading: false, error: null, live: true }),
}));

const json = (status: number, body: unknown) =>
  new Response(body === null ? null : JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const err = (status: number, code: string, message = code) => json(status, { error: { code, message } });
type Handler = (req: Request) => Response | Promise<Response>;
function mockFetch(routes: Record<string, Handler>) {
  const fn = vi.fn(async (input: Request | string | URL, init?: RequestInit) => {
    const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"), init);
    const url = new URL(req.url, "http://localhost");
    const h = routes[`${req.method} ${decodeURIComponent(url.pathname)}`];
    return h ? h(req) : err(404, "not_found", "nf");
  });
  vi.stubGlobal("fetch", fn);
  return fn;
}
function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const ev = (seq: number, type: string, payload: Record<string, unknown>): TranscriptEvent => ({
  seq, type, ts: "2026-01-01T00:00:00Z", payload,
});

const aa = (over: Record<string, unknown> = {}) => ({
  id: "x1", workspace_id: "w", agent_id: "a", action_key: "send_email", summary: "Send <b>mail</b> to bob", status: "pending",
  expires_at: "2026-02-01T00:00:00Z", created_at: "2026-01-01T00:00:00Z", ...over,
});

describe("action approvals", () => {
  it("renders the summary as plain text, decides, and refreshes on 409 already_decided", async () => {
    let decided: unknown = null;
    let lists = 0;
    mockFetch({
      "GET /v1/workspaces/w/approvals": () => json(200, { items: [], next_cursor: null }),
      "GET /v1/workspaces/w/action-approvals": () => {
        lists++;
        return json(200, { items: lists === 1 ? [aa()] : [], next_cursor: null });
      },
      "POST /v1/action-approvals/x1/decision": async (r) => {
        decided = await r.json();
        return err(409, "already_decided", "done");
      },
    });
    wrap(<ApprovalsView wsId="w" />);
    expect(await screen.findByText("Send <b>mail</b> to bob")).toBeTruthy();
    expect(document.querySelector("b")).toBeNull();
    fireEvent.click(screen.getAllByRole("button", { name: "Approve" }).at(-1)!);
    await waitFor(() => expect(decided).toEqual({ decision: "approve" }));
    await waitFor(() => expect(lists).toBeGreaterThanOrEqual(2));
    expect(await screen.findByText("No pending action approvals")).toBeTruthy();
  });

  it("history queries by status", async () => {
    let status: string | null = null;
    mockFetch({
      "GET /v1/workspaces/w/approvals": () => json(200, { items: [], next_cursor: null }),
      "GET /v1/workspaces/w/action-approvals": (r) => {
        status = new URL(r.url).searchParams.get("status");
        return json(200, { items: [aa({ status: "executed", decided_by: { id: "u", display_name: "Ann" } })], next_cursor: null });
      },
    });
    wrap(<ApprovalsView wsId="w" />);
    fireEvent.click(screen.getByRole("tab", { name: "History" }));
    fireEvent.change(await screen.findByLabelText("Action status"), { target: { value: "executed" } });
    await waitFor(() => expect(status).toBe("executed"));
    expect(await screen.findByText("Ann")).toBeTruthy();
  });

  it("card is plain text", () => {
    render(<ul><ActionApprovalCard a={aa() as never} pending={false} onDecide={() => {}} /></ul>);
    expect(screen.getByText("Send <b>mail</b> to bob")).toBeTruthy();
  });
});

function pane() {
  return wrap(<ChatPane agentId="a1" wsId="w" base="/o/x/w/y" convId="c1" direct={null} />);
}
const att = { id: "att1", conversation_id: "c1", direction: "in", status: "pending", filename: "pic.png", content_type: "image/png", size_bytes: 2048, created_at: "2026-01-01T00:00:00Z" };

describe("chat attachments", () => {
  it("uploads, shows a removable chip and sends attachment_ids", async () => {
    let sent: unknown = null;
    mockFetch({
      "POST /v1/conversations/c1/attachments": () => json(201, att),
      "POST /v1/conversations/c1/messages": async (r) => {
        sent = await r.json();
        return json(202, { turn_id: "t1", status: "queued" });
      },
    });
    pane();
    const input = screen.getByLabelText("Attach files");
    fireEvent.change(input, { target: { files: [new File(["x"], "pic.png", { type: "image/png" })] } });
    expect(await screen.findByText(/pic\.png \(2\.0 KiB\)/)).toBeTruthy();
    fireEvent.change(screen.getByLabelText("Message"), { target: { value: "look" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(sent).toEqual({ text: "look", attachment_ids: ["att1"] }));
    await waitFor(() => expect(screen.queryByText(/pic\.png/)).toBeNull());
  });

  it("remove chip drops it", async () => {
    mockFetch({ "POST /v1/conversations/c1/attachments": () => json(201, att) });
    pane();
    fireEvent.change(screen.getByLabelText("Attach files"), { target: { files: [new File(["x"], "pic.png")] } });
    fireEvent.click(await screen.findByRole("button", { name: "Remove pic.png" }));
    expect(screen.queryByText(/pic\.png/)).toBeNull();
  });

  it("shows the size-limit error on 413", async () => {
    mockFetch({ "POST /v1/conversations/c1/attachments": () => err(413, "payload_too_large") });
    pane();
    fireEvent.change(screen.getByLabelText("Attach files"), { target: { files: [new File(["x"], "big.bin")] } });
    expect((await screen.findByRole("alert")).textContent).toContain("too large");
    expect(screen.queryByRole("list", { name: "Pending attachments" })).toBeNull();
  });

  it("shows too_many_attachments on send 422", async () => {
    mockFetch({
      "POST /v1/conversations/c1/attachments": () => json(201, att),
      "POST /v1/conversations/c1/messages": () => err(422, "too_many_attachments", "x"),
    });
    pane();
    fireEvent.change(screen.getByLabelText("Attach files"), { target: { files: [new File(["x"], "pic.png")] } });
    await screen.findByText(/pic\.png/);
    fireEvent.change(screen.getByLabelText("Message"), { target: { value: "hi" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    expect((await screen.findByRole("alert")).textContent).toContain("Too many attachments");
  });
});

describe("transcript attachments", () => {
  const view = (e: TranscriptEvent) => {
    const state = applyEvents(emptyTranscript, [e]);
    return wrap(<TranscriptView state={state} wsId="w" base="/b" convId="c1" />);
  };
  it("renders SVG only as a download link, never inline or as an image", () => {
    view(ev(1, "attachment", { attachment_id: "s1", filename: "x.svg", content_type: "image/svg+xml", size: 10 }));
    // (the UI's own decorative icons are `svg[aria-hidden]`; attachment content is never inlined)
    expect(document.querySelector('img, svg:not([aria-hidden="true"]), object, iframe, embed')).toBeNull();
    expect(screen.getByRole("link", { name: "x.svg" }).getAttribute("href")).toBe("/v1/attachments/s1/content");
  });
  it("raster images get an <img> of the download URL only", () => {
    view(ev(1, "attachment", { attachment_id: "p1", filename: "p.png", content_type: "image/png", size: 10 }));
    expect(document.querySelector("img")?.getAttribute("src")).toBe("/v1/attachments/p1/content");
  });
  it("HTML is a plain link", () => {
    view(ev(1, "attachment", { attachment_id: "h1", filename: "a.html", content_type: "text/html", size: 10 }));
    expect(document.querySelector("img, iframe")).toBeNull();
    expect(screen.getByRole("link", { name: "a.html" })).toBeTruthy();
  });
});

describe("app turn cap", () => {
  it("Continue calls the endpoint; hidden once a reset notice exists", async () => {
    let called = 0;
    mockFetch({ "POST /v1/conversations/c1/continue": () => { called++; return json(200, { id: "c1" }); } });
    const cap = ev(1, "notice", { code: "app_turn_cap_reached", text: "Paused" });
    const state = applyEvents(emptyTranscript, [cap]);
    const { unmount } = wrap(<TranscriptView state={state} wsId="w" base="/b" convId="c1" />);
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    await waitFor(() => expect(called).toBe(1));
    unmount();
    const reset = ev(2, "notice", { code: "app_turn_cap_reset", text: "Reset" });
    const state2 = applyEvents(emptyTranscript, [cap, reset]);
    wrap(<TranscriptView state={state2} wsId="w" base="/b" convId="c1" />);
    expect(screen.queryByRole("button", { name: "Continue" })).toBeNull();
  });
});
