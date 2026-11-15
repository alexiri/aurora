import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ApprovalsView } from "@/components/approvals/approvals-view";
import { applyApprovalEvent } from "@/lib/api/approvals";

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

const mk = (id: string, over: Record<string, unknown> = {}) => ({
  id, org_id: "o", workspace_id: "w1", status: "pending", method: "POST", host: "api.x.com", path: "/v1",
  created_at: `2026-01-01T00:00:0${id.length}Z`, deadline: "2099-01-01T00:00:00Z",
  agent: { id: "ag1", handle: "bot", name: "Bot" },
  summary: { title: `Send ${id}`, fields: [{ label: "To", value: "bob" }], source: "generic" },
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
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
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
    act(() => FakeES.last.emit("approval", { event: "created", approval: mk("a22", { summary: { title: "<script>alert(1)</script>", fields: [], source: "generic" } }) }));
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
    fireEvent.click(await screen.findByRole("button", { name: "Approve" }));
    expect(await screen.findByText(/already decided by someone else/i)).toBeInTheDocument();
  });

  it("batch decision posts agent_id", async () => {
    const batch = { id: "b1", label: "Send emails", total: 2 };
    routes["GET /v1/workspaces/w1/approvals"] = () =>
      json(200, { items: [mk("a1", { batch }), mk("a22", { batch })], next_cursor: null });
    routes["POST /v1/approvals/batches/b1/decision"] = () => json(200, { decided: 2 });
    show();
    fireEvent.click(await screen.findByRole("button", { name: "Approve all 2" }));
    await waitFor(() =>
      expect(calls.find((c) => c.key === "POST /v1/approvals/batches/b1/decision")?.body).toEqual({
        agent_id: "ag1",
        decision: "approve",
      }),
    );
  });
});
