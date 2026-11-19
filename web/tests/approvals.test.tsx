import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApprovalsView } from "@/components/approvals/approvals-view";
import { applyApprovalEvent } from "@/lib/api/approvals";
import { json } from "./support/api";

const mk = (id: string, over: Record<string, unknown> = {}) => ({
  id, org_id: "o", workspace_id: "w1", status: "pending", method: "POST", host: "api.x.com", path: "/v1",
  created_at: `2026-01-01T00:00:0${id.length}Z`, deadline: "2099-01-01T00:00:00Z",
  agent: { id: "ag1", handle: "bot", name: "Bot" },
  summary: { title: `Send ${id}`, fields: [{ label: "To", value: "bob" }], source: "generic" },
  sentence: { agent: { id: "ag1", name: "Bot" }, action: `Send ${id}` },
  can_always_allow: false,
  ...over,
});

class FakeES {
  static last: FakeES;
  listeners: Record<string, ((e: MessageEvent) => void)[]> = {};
  onopen: (() => void) | null = null;
  onerror: (() => void) | null = null;
  constructor(public url: string) {
    FakeES.last = this;
  }
  addEventListener(t: string, fn: (e: MessageEvent) => void) {
    (this.listeners[t] ??= []).push(fn);
  }
  emit(t: string, data: unknown) {
    this.listeners[t]?.forEach((f) => f({ data: JSON.stringify(data) } as MessageEvent));
  }
  close() {}
}

let routes: Record<string, () => Response>;
let calls: { key: string; body: unknown }[];

beforeEach(() => {
  calls = [];
  routes = {};
  vi.stubGlobal("EventSource", FakeES);
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request) => {
      const url = new URL(input.url);
      const key = `${input.method} ${url.pathname}`;
      const text = input.method === "POST" ? await input.text() : "";
      calls.push({ key, body: text ? JSON.parse(text) : null });
      return routes[key]?.() ?? json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
});

function show() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={qc}>
      <ApprovalsView wsId="w1" />
    </QueryClientProvider>,
  );
}

describe("approvals", () => {
  it("reducer adds, and removes on decided", () => {
    const a = mk("a1") as never;
    const d = applyApprovalEvent({ items: [] }, { event: "created", approval: a });
    expect(d?.items).toHaveLength(1);
    expect(applyApprovalEvent(d, { event: "decided", approval: { ...(a as object), status: "approved" } as never })?.items).toHaveLength(0);
    // (FIX-109) A freeze-abandoned approval leaves the pending list like an expired one.
    expect(applyApprovalEvent(d, { event: "abandoned", approval: { ...(a as object), status: "abandoned" } as never })?.items).toHaveLength(0);
  });

  it("SSE events update the list and escape summary text", async () => {
    routes["GET /v1/workspaces/w1/approvals"] = () => json(200, { items: [mk("a1")], next_cursor: null });
    show();
    await screen.findByText("Send a1");
    act(() => FakeES.last.emit("approval", { event: "created", approval: mk("a22", { summary: { title: "<script>alert(1)</script>", fields: [], source: "generic" }, sentence: { agent: { id: "ag1", name: "Bot" }, action: "<script>alert(1)</script>" } }) }));
    expect(await screen.findByText("<script>alert(1)</script>")).toBeInTheDocument();
    expect(document.querySelector("script")).toBeNull();
    act(() => FakeES.last.emit("approval", { event: "decided", approval: mk("a1", { status: "approved" }) }));
    await waitFor(() => expect(screen.queryByText("Send a1")).not.toBeInTheDocument());
    act(() => FakeES.last.emit("approval", { event: "created", approval: mk("zz", { workspace_id: "other" }) }));
    expect(screen.queryByText("Send zz")).not.toBeInTheDocument();
  });

  it("shows already-decided on 409", async () => {
    routes["GET /v1/workspaces/w1/approvals"] = () => json(200, { items: [mk("a1")], next_cursor: null });
    routes["POST /v1/approvals/a1/decision"] = () => json(409, { error: { code: "already_decided", message: "x" } });
    show();
    fireEvent.click(await screen.findByRole("button", { name: "Allow once" }));
    expect(await screen.findByText(/already decided by someone else/i)).toBeInTheDocument();
  });

  it("reads as a sentence with the app and the account, and offers three answers where Always allow is possible", async () => {
    const q = mk("a1", {
      sentence: { agent: { id: "ag1", name: "Daneel" }, action: "comment on an issue", app: { app_id: "linear", name: "Linear" }, account: { ref: { type: "connection", id: "c1" }, name: "Bob's Linear" } },
      can_always_allow: true,
      asked_by: { kind: "access", agent_id: "ag1", connection_id: "c1", tool_id: "comment" },
    });
    routes["GET /v1/workspaces/w1/approvals"] = () => json(200, { items: [q], next_cursor: null });
    show();
    const heading = await screen.findByText("comment on an issue");
    expect(heading.closest("p")?.textContent).toBe("Daneel wants to comment on an issue in Linear");
    expect(screen.getByText("Bob's Linear")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Allow once" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Block" })).toBeInTheDocument();
    routes["POST /v1/approvals/a1/decision"] = () =>
      json(200, { ...q, status: "approved", always_applied: { tool: { id: "comment", name: "Comment on an issue" }, also_approved: 2 } });
    fireEvent.click(screen.getByRole("button", { name: "Always allow" }));
    await waitFor(() => expect(calls.find((c) => c.key === "POST /v1/approvals/a1/decision")?.body).toEqual({ decision: "approve", always: true }));
    const note = await screen.findByTestId("always-allowed");
    expect(note.textContent).toContain("Daneel will no longer ask before it wants to comment on an issue in Linear.");
    expect(note.textContent).toContain("2 other waiting requests of the same kind went ahead too.");
    expect(within(note).getByRole("link").getAttribute("href")).toContain("/agents/bot?tab=access");
  });

  it("a question asked by a rule names the rule and offers no Always allow", async () => {
    const q = mk("a1", { asked_by: { kind: "rule", rule_id: "r1", rule_name: "Ask on status.example.org", scope: "workspace" }, can_always_allow: false });
    routes["GET /v1/workspaces/w1/approvals"] = () => json(200, { items: [q], next_cursor: null });
    show();
    expect(await screen.findByTestId("asked-by-rule")).toHaveTextContent("Asked by the rule Ask on status.example.org");
    expect(screen.queryByRole("button", { name: "Always allow" })).toBeNull();
    expect(screen.getByRole("button", { name: "Allow once" })).toBeInTheDocument();
  });

  it("(P5-TOOLASK) a question held by the app's own default says the app asks, names no rule and offers no Always allow", async () => {
    const q = mk("a1", { asked_by: { kind: "package", app_id: "caldav", tool_id: "send_scheduling_message" }, can_always_allow: false });
    routes["GET /v1/workspaces/w1/approvals"] = () => json(200, { items: [q], next_cursor: null });
    show();
    expect(await screen.findByTestId("asked-by-package")).toHaveTextContent("This app asks before this action.");
    expect(screen.queryByTestId("asked-by-rule")).toBeNull();
    expect(screen.queryByRole("button", { name: "Always allow" })).toBeNull();
    expect(screen.getByRole("button", { name: "Allow once" })).toBeInTheDocument();
  });

  it("an empty list says nothing is waiting, in the words of the page", async () => {
    routes["GET /v1/workspaces/w1/approvals"] = () => json(200, { items: [], next_cursor: null });
    show();
    expect(await screen.findByText("Nothing is waiting for you.")).toBeInTheDocument();
    expect(screen.getByText("Agents ask here when their access is Ask before changes, or when a rule says so.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("tab", { name: "History" }));
    expect(await screen.findByText("No questions have been answered yet.")).toBeInTheDocument();
  });

  it("batch decision posts agent_id", async () => {
    const batch = { id: "b1", label: "Send emails", total: 2 };
    routes["GET /v1/workspaces/w1/approvals"] = () =>
      json(200, { items: [mk("a1", { batch }), mk("a22", { batch })], next_cursor: null });
    routes["POST /v1/approvals/batches/b1/decision"] = () => json(200, { decided: 2 });
    show();
    fireEvent.click(await screen.findByRole("button", { name: "Allow all 2 once" }));
    await waitFor(() =>
      expect(calls.find((c) => c.key === "POST /v1/approvals/batches/b1/decision")?.body).toEqual({
        agent_id: "ag1",
        decision: "approve",
      }),
    );
  });
});
