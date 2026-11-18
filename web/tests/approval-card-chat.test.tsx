// (P5-UX-16, C-982, C-983) The approval card in the conversation: the question while it waits, who it waits for when
// the viewer may not answer, and the answer the transcript carries (Always allow says what it changed).
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ApprovalCard } from "@/components/chat/approval-card";
import type { TranscriptEvent } from "@/lib/api/conversations";
import { applyEvents, emptyTranscript } from "@/lib/chat/transcript";

const json = (status: number, body: unknown) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

const ev = (seq: number, type: string, payload: Record<string, unknown>): TranscriptEvent => ({ seq, type, ts: "2026-01-01T00:00:00Z", payload });

const question = (over: Record<string, unknown> = {}) => ({
  id: "ap1", org_id: "o", workspace_id: "w", status: "pending", method: "POST", host: "api.linear.app", path: "/graphql",
  created_at: "2026-01-01T00:00:00Z", deadline: "2099-01-01T00:00:00Z", parked: false,
  agent: { id: "ag1", handle: "daneel", name: "Daneel" },
  summary: { title: "comment on an issue", fields: [{ label: "Issue", value: "PLAT-412" }], source: "template" },
  sentence: { agent: { id: "ag1", name: "Daneel" }, action: "comment on an issue", app: { app_id: "linear", name: "Linear" }, account: { ref: { type: "connection", id: "c1" }, name: "Bob's Linear" } },
  asked_by: { kind: "access", agent_id: "ag1", connection_id: "c1", tool_id: "comment" },
  can_always_allow: true,
  ...over,
});

function stub(role: "owner" | "member", items: unknown[], decisions: unknown[] = []) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request) => {
      const url = new URL(input.url, "http://localhost");
      if (input.method === "POST") {
        decisions.push(JSON.parse(await input.text()));
        return json(200, { ...(items[0] as object), status: "approved" });
      }
      if (url.pathname === "/v1/workspaces/w") return json(200, { my_role: role, contacts: { owners: [{ name: "Ada Lovelace" }, { name: "Alan Turing" }] } });
      if (url.pathname === "/v1/workspaces/w/approvals") return json(200, { items, next_cursor: null });
      return json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
}

function show(events: TranscriptEvent[]) {
  const state = applyEvents(emptyTranscript, events);
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <ApprovalCard ev={events[0]} state={state} wsId="w" />
    </QueryClientProvider>,
  );
}

const pendingEvent = ev(1, "approval.pending", {
  approval_id: "ap1", summary: "Daneel wants to comment on an issue in Linear", parked: false,
  agent_name: "Daneel", agent_handle: "daneel", action: "comment on an issue", app_name: "Linear",
});

describe("the approval card in the conversation", () => {
  it("shows the sentence and three answers, and Always allow sends always: true", async () => {
    const decisions: unknown[] = [];
    stub("owner", [question()], decisions);
    show([pendingEvent]);
    const action = await screen.findByText("comment on an issue");
    expect(action.closest("p")?.textContent).toBe("Daneel wants to comment on an issue in Linear");
    expect(screen.getByText("Bob's Linear")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Allow once" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Block" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Always allow" }));
    await waitFor(() => expect(decisions).toEqual([{ decision: "approve", always: true }]));
  });

  it("offers no Always allow when the server does not", async () => {
    stub("owner", [question({ can_always_allow: false })]);
    show([pendingEvent]);
    await screen.findByRole("button", { name: "Allow once" });
    expect(screen.queryByRole("button", { name: "Always allow" })).toBeNull();
  });

  it("says who it waits for when the viewer may not answer", async () => {
    stub("member", [question({ can_always_allow: false })]);
    show([pendingEvent]);
    expect(await screen.findByText("Waiting for Ada Lovelace or Alan Turing")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Allow once" })).toBeNull();
  });

  it("settles to one line after Always allow and says what changed", async () => {
    stub("owner", []);
    const decided = ev(2, "approval.decided", {
      approval_id: "ap1", summary: "Daneel wants to comment on an issue in Linear", decision: "approve", decided_by_label: "Bob",
      agent_name: "Daneel", agent_handle: "daneel", action: "comment on an issue", app_name: "Linear",
      always: { tool: { id: "comment", name: "Comment on an issue" }, also_approved: 1 },
    });
    show([pendingEvent, decided]);
    expect(await screen.findByText(/Approved by Bob/)).toBeInTheDocument();
    const note = screen.getByTestId("always-allowed");
    expect(note.textContent).toContain("Daneel will no longer ask before it wants to comment on an issue in Linear.");
    expect(note.textContent).toContain("1 other waiting request of the same kind went ahead too.");
    expect(within(note).getByRole("link").getAttribute("href")).toContain("/agents/daneel?tab=access");
  });

  it("shows an expired question as expired", async () => {
    stub("owner", []);
    show([pendingEvent, ev(3, "approval.expired", { approval_id: "ap1", reason: "deadline" })]);
    expect(await screen.findByText("Expired")).toBeInTheDocument();
  });
});
